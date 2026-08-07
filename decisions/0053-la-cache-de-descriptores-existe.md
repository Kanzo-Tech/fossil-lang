# ADR 0053: La caché de descriptores existe, se clava por URI, y su token no es un hash

**Date:** 2026-08-07
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Ejecuta la fase F3 de ADR-0046 (la fila `content_hash` de la tabla de §Contexto y la regla
de extensión de §5). Reclava ADR-0037 §1 y §3 —`source_name` como clave y `content_hash` como
nombre del campo—. Las medidas de `stat` frente a lectura de bytes son de este documento.

## Contexto

ADR-0037 declaró tres cosas sobre el descriptor inferido, y ninguna llegó a existir.

**La primera: `content_hash` «dirige la invalidación de Salsa».** En el árbol era `String::new()` en
todos los sitios de producción — el escritor nativo (`fossil-engine`), el derivado de ShEx
(`fossil-descriptors-input`), el constructor de conveniencia (`InferredDescriptor::empty`) y el host
del navegador (`@fossil-lang/introspect`). Nada lo escribía, y por tanto nada lo comparaba: cada
compilación volvía a correr el `DESCRIBE` sobre cada fuente. Y el único mecanismo que el documento
ofrecía para rellenarlo —`derived_content_hash()`, que hashea la lista de columnas— **no puede
servir para lo que se le pedía**: se calcula a partir del resultado de la introspección, así que
para saber si hay que introspeccionar hay que haber introspeccionado ya. Existía para que un campo
vacío pareciera lleno.

**La segunda: `register_inferred_descriptor` en el trait `System`, con `panic!` por defecto.** Un
`System` que no participara en el juego no fallaba al compilar: fallaba al ejecutarse, en la primera
escritura, con un mensaje que sólo dice que el método no está.

**La tercera: la clave es el nombre del binding.** `users := io.csv("data/users.csv")` se archivaba
bajo `"users"`. Tres consecuencias, todas incorrectas: dos bindings sobre el mismo fichero se
introspeccionan dos veces; renombrar el binding tira la entrada; y —la que importa— **la frescura es
una propiedad del fichero, y el nombre del binding no nombra un fichero.**

Y había una cuarta cosa que nadie declaró: la tabla estaba implementada **tres veces**. `NativeSystem`,
`EngineSystem` y `WasmSystem` llevaban cada uno su `Mutex<HashMap<SmolStr, InferredDescriptor>>`
privado y su par de métodos para alcanzarlo, idénticos salvo en el comentario.

El orden de magnitud es el que pone F3 primero: **los programas son pequeños y las fuentes no.** Un
`.fossil` son decenas de líneas; el CSV al que apunta puede ser de gigabytes. El trabajo caro de una
compilación es leer la fuente, y es exactamente el trabajo que casi nunca hace falta repetir.

## Decisión

### 1. Una tabla, alcanzada por un accesor

`DescriptorCache` es un struct concreto en `fossil-descriptors-input`: el `HashMap`, su cerrojo, la
regla de frescura y un contador de introspecciones. El trait `System` pierde los dos métodos y gana
uno, `descriptors() -> Option<&DescriptorCache>`.

Es la forma de §5 de ADR-0046 en lo que aplica a una tabla de datos: **ambiente en el contexto, y
nunca parte de la clave de una consulta.** Leerla desde dentro de una consulta trackeada no registra
dependencia ni dispara invalidación, igual que `read_file`. Lo que **no** se copia de rustc es el
`fn` plano, y la razón es que aquí no hay nada que despachar: `Providers` es una tabla de
comportamiento y ésta es una tabla de datos. Un puntero a función que devolviera descriptores sería
la indirección sin el motivo.

**Y el `panic!` desaparece porque desaparece el hueco donde estaba**, no porque se haya sustituido
por un `no-op`. `None` es una respuesta real y la da un host real: el ejecutor de `fossil-df-wasm`
recibe sus esquemas en el plan que le entregan y no tiene nada que cachear.

### 2. La clave es el URI que escribe el programa

`InferredDescriptor.source_name` pasa a ser `InferredDescriptor.uri`, y es la cadena que hay dentro
de `io.csv("…")` — **no** el localizador resuelto.

El URI resuelto sería el mejor identificador del fichero, y no sirve: la resolución necesita los
`@conn` del `--creds-stdin` y el directorio del programa, y el checker no tiene ninguno de los dos.
El URI tal cual se escribe es la única cadena que el host y el checker ven igual. El `DefMap` ya lo
lleva (`SourceEntry.uri`, que `fossil-mir::lower` necesitaba para `Op::Source`), así que el consumidor
lo alcanza sin que cruce ningún dato nuevo una frontera de consulta.

Un URI escrito es relativo casi siempre, y `"users.csv"` nombra ficheros distintos en directorios
distintos. **El ámbito en el que un URI escrito es inequívoco es el directorio del programa**, así
que ése es el ámbito de la tabla: `fossil-engine` guarda una caché por directorio de programa. No
puede colgar del `FossilDb` —`open_db` construye uno nuevo con `Storage` vacío en cada llamada, el
hallazgo de ADR-0046 §6—, con lo que `check` y el `run` que le sigue tirarían la tabla justo entre
las dos veces que la queremos.

### 3. El token de frescura nativo es `mtime` + tamaño, y el campo deja de llamarse hash

`content_hash` pasa a ser `freshness_token`. Sigue siendo opaco y sigue siendo del host; lo que
cambia es que el nombre ya no promete un algoritmo concreto. La caché **compara** el token; nadie lo
interpreta.

Las tres candidatas que pedía el plan, y por qué gana la que gana:

- **Hash del contenido.** Es el token semánticamente correcto y es el que no podemos pagar: exige
  leer la fuente entera para decidir si hace falta leer la fuente. Sobre el corpus del millón eso es
  leer cientos de megas para ahorrarse un `DESCRIBE` que lee las primeras filas. **La caché se
  volvería más cara que lo que cachea**, que es la única forma de fallar en la que este documento
  podía caer.
- **ETag.** Sólo existe en HTTP, exige un viaje de red (`HEAD`) para leerlo, y dos de las tres
  familias de fuente que el host nativo toca —fichero local y `s3://` a través de DuckDB— no lo dan
  en la capa en la que estamos. En el navegador **sí** es la elección barata, y por eso el campo es
  opaco: `@fossil-lang/introspect` acepta un `freshness(ref, url)` del host y lo estampa tal cual.
- **`mtime` + tamaño.** Dos campos de un `stat`, sin leer un byte de la fuente. Es lo que se
  implementa en el host nativo.

Los modos de fallo de `mtime`, dichos antes de cobrarlos. Puede decir «cambió» cuando no cambió —un
`touch`— y eso cuesta un `DESCRIBE` de más y ninguna respuesta incorrecta. Puede decir «no cambió»
cuando sí cambió, y **ése es el único que da una respuesta incorrecta**: exige restaurar un fichero
preservando `mtime` *y* dando exactamente el mismo tamaño. Emparejar el tamaño con el `mtime` es lo
que estrecha esa ventana, y es barato porque el mismo `stat` trae los dos.

**Y el token vacío es una respuesta, no un hueco.** `is_fresh` lo lee como «nunca fresco»: un
`https://` o un `s3://` que este host no puede `stat`-ear se re-introspecciona en cada compilación.
Es la respuesta honesta para un objeto al que habría que ir por red a preguntarle.

`derived_content_hash()` se borra. No tenía llamadores fuera de su propio test, y lo que hacía no
era lo que su nombre decía.

## Consecuencias

**Lo que se vuelve más fácil.** Una compilación cuyas fuentes no se han movido no abre siquiera una
conexión de DuckDB. Dos bindings sobre un fichero se leen una vez. Y «¿esto re-introspeccionó?» pasa
a ser una pregunta con un número por respuesta: `DescriptorCache::registrations()` sólo se mueve
cuando hubo lectura de verdad, así que el done-when de F3 se comprueba contando y no cronometrando
(`fossil-engine`,
`a_source_is_re_introspected_when_it_changes_and_not_when_it_does_not`).

**Lo que se paga.** El cable cambia dos veces en dos días: ADR-0047 renombró el deletreo de las
primitivas y esto renombra dos campos del mismo objeto. No hay alias ni periodo de gracia — es la
regla del repo y los paquetes no están publicados. Un host que ya enviaba `source_name` deja de
compilar contra `@fossil-lang/{introspect,wasm}`, que es exactamente lo que queremos que le pase.

**Lo que queda abierto, y es lo incómodo.** La caché por directorio de programa **no se vacía nunca**:
un host de vida larga acumula una tabla por cada directorio que haya compilado. Está acotado por el
número de directorios y no por el de compilaciones, y una entrada vieja no miente —el token la
delata—, pero es memoria que sólo crece. La medida que decidiría si hace falta desalojar no está
tomada, y no lo estará hasta que exista un host de vida larga que compile muchos directorios.

**Lo que este documento no prueba.** Que saltarse la introspección ahorre tiempo de reloj. Lo que se
mide aquí son lecturas de la fuente, no milisegundos; la afirmación de que ahí está el trabajo caro
viene de la forma del problema (programas pequeños, fuentes grandes) y de ADR-0046, no de un
cronómetro sobre este cambio.
