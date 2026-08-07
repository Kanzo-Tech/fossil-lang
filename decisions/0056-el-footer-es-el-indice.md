# ADR 0056: El footer es el índice, y la aritmética de 566 kB era optimista al revés

**Date:** 2026-08-07
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Desbloquea F7 de ADR-0046 y comprueba sus tres afirmaciones de §9. Revisa ADR-0042 §3.1 en
un punto (una tesela no tiene por qué ser un fichero) y lo confirma en todo lo demás. El arnés es
`crates/fossil-df/examples/tile_layout.rs`; los números de aquí salen de él, en este árbol, el
2026-08-07.

## Contexto

F7 estaba bloqueada por una medición que faltaba: escribir cinco millones de vértices de las dos
formas —un fichero por tesela, o un fichero con row groups de 4.096— y contar footer, peticiones y
bytes por ventana. La cifra que circulaba, 566 kB de footer, era **aritmética**: alguien la calculó.

Y el árbol tiene **dos escritores**. El nativo es `COPY` de `DuckDB`
(`crates/fossil-runtime/src/layout.rs`), que emite `chunk{k}.parquet` por tesela. El compartido con
el navegador es `batches_to_parquet` (`crates/fossil-df/src/files.rs`), que hasta este ADR no
configuraba **ni una sola opción de Parquet** — ni tamaño de row group ni codificación.

## La medición

Cinco millones de vértices, 2.048 clusters, tesela de 4.096 filas → 1.221 teselas. Maquetación
`cluster_layout` y renumerado Morton calcados de `fossil-runtime`; páginas sin comprimir; esquema de
dibujo (`dense_id`, `x`, `y`, `cluster_id`) y, donde se dice, el de seis columnas de ADR-0042 §3.4.

```
cargo run --release -p fossil-df --example tile_layout -- --rows 5000000 --dir /tmp/f7
duckdb -c "COPY (SELECT * FROM read_parquet('/tmp/f7/corpus.parquet') ORDER BY dense_id)
             TO '/tmp/f7/duck_single.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 4096, COMPRESSION UNCOMPRESSED);
           COPY (SELECT *, (dense_id >> 12) AS tile FROM read_parquet('/tmp/f7/corpus.parquet'))
             TO '/tmp/f7/duck_tiles' (FORMAT PARQUET, PARTITION_BY (tile), OVERWRITE_OR_IGNORE, COMPRESSION UNCOMPRESSED);"
cargo run --release -p fossil-df --example tile_layout -- --rows 5000000 --dir /tmp/f7
```

### 1. Footer y bytes

| disposición | ficheros | row groups | footer total | por tesela | índice de páginas | almacenado |
|---|---|---|---|---|---|---|
| A · arrow-rs, un fichero por tesela | 1.221 | 1.221 | 1.150.490 B | **942 B** | 174.603 B | 76,46 MB |
| B · arrow-rs, un fichero, row groups de 4.096 | 1 | 1.221 | **496.373 B** | **406 B** | 179.419 B | 75,81 MB |
| C · arrow-rs, un fichero, diccionario apagado | 1 | 1.221 | 437.532 B | 358 B | 185.526 B | 80,73 MB |
| D · arrow-rs, un fichero, diccionario por columna | 1 | 1.221 | 448.781 B | 367 B | 183.063 B | **60,78 MB** |
| B′ · `DuckDB` COPY, un fichero, `ROW_GROUP_SIZE 4096` | 1 | 1.221 | 329.727 B | 270 B | **0 B** | 60,57 MB |
| A′ · `DuckDB` COPY, `PARTITION_BY tile` | 1.221 | 1.221 | 494.544 B | 405 B | **0 B** | 60,74 MB |

**Una tesela es un row group: confirmado**, 1.221 y 1.221, sin sorpresas en el remanente.

**Los 566 kB eran altos.** El footer de B son **496.373 B**. La estimación estaba un 14% por encima
de lo que se mide, y por la razón habitual: multiplicaba el footer de un fichero suelto por el número
de teselas, y el footer de un fichero lleva el esquema una vez, no 1.221 veces.

**Y los 710 bytes son de otra cosa.** El footer de un fichero-tesela son **942 B** con cuatro
columnas y **1.358 B** con seis; el que sale a 710 es el reparto por row group del fichero único con
seis columnas — **665 B** a cinco millones, **651 B** al millón, que es el corpus sobre el que se
tomó. Es decir: la cifra de ADR-0046 §9 medía **B**, no A, y al leerla como «el footer de una tesela»
subestima el coste de A en 2,0×. Y es independiente de N — 942/943 B y 1.358/1.359 B al millón y a
los cinco.

### 2. Peticiones y bytes por ventana

Nueve cuadrados de semiancho 2.472 sobre un lienzo de 82.201, uno por cada noveno de `dense_id`,
dimensionado sobre el central para contener 20.000 vértices.

| # | dibujados | necesarias | seleccionadas | sobrelectura |
|---|---|---|---|---|
| 1 | 35.122 | 20 | 23 | 1,15× |
| 2 | 35.049 | 21 | 25 | 1,19× |
| 3 | 35.012 | 21 | 24 | 1,14× |
| 4 | 34.988 | 21 | 23 | 1,10× |
| 5 | 20.003 | 10 | 11 | 1,10× |
| 6 | 35.009 | 21 | 23 | 1,10× |
| 7 | 35.242 | 21 | 24 | 1,14× |
| 8 | 35.008 | 21 | 24 | 1,14× |
| 9 | 35.091 | 20 | 24 | 1,20× |

**Seleccionan 11–25 teselas donde hacen falta 10–21, sobrelectura 1,10×–1,20×, media 1,14×.**
ADR-0046 §9 decía 14–23 sobre 13–21 y 1,05×–1,21×. **Se reproduce**, con la banda de sobrelectura
dentro de la suya y las dos colas explicadas por la ventana 5, que es la que fijó el tamaño.

| disposición | peticiones/ventana | bytes/ventana | sólo las necesarias | índice, una vez |
|---|---|---|---|---|
| A · arrow-rs, un fichero por tesela | **22,3** | 1,40 MB | 1,23 MB | 1,33 MB (1.221 footers) |
| B · arrow-rs, un fichero, RG 4.096 | **5,6** | 1,38 MB | 1,21 MB | 675,8 kB (1 footer) |
| D · arrow-rs, diccionario por columna | 5,6 | **1,10 MB** | 963,6 kB | 631,8 kB (1 footer) |
| B′ · `DuckDB`, un fichero | 5,6 | 1,10 MB | 964,2 kB | 329,7 kB (1 footer) |
| A′ · `DuckDB`, `PARTITION_BY` | 22,3 | 1,11 MB | 973,1 kB | 494,5 kB (1.221 footers) |

Una petición es **un fichero** cuando la tesela es un fichero, y **un tramo maximal de row groups
consecutivos** cuando no: dentro de un fichero, teselas contiguas se piden en un solo rango. Las 22,3
teselas seleccionadas se agrupan en **5,6 tramos**, así que la frontera de fichero es lo único que
impide fundirlas. **Es el hallazgo entero del apartado: un fichero por tesela cuesta 4× las
peticiones y no ahorra un byte.**

Las columnas `necesarias`/`seleccionadas` son idénticas en las cinco disposiciones, y el arnés lo
comprueba con un `assert` por ventana: **las cajas de `DuckDB` y las de arrow-rs seleccionan
exactamente el mismo conjunto**.

### 3. El índice de páginas: las dos afirmaciones del plan son ciertas y una es irrelevante

| escritor | `ColumnIndex` + `OffsetIndex` | bytes |
|---|---|---|
| arrow-rs (`parquet` 58), props por defecto | **sí** | 174.603 |
| arrow-rs, row groups de 4.096 | **sí** | 179.419 |
| `DuckDB` 1.5.3, `COPY … ROW_GROUP_SIZE 4096` | **no** | 0 |
| `DuckDB` 1.5.3, `COPY … PARTITION_BY` | **no** | 0 |

Leído con `PageIndexPolicy::Optional`, que devuelve lo que hay. `parquet_metadata()` de `DuckDB` ni
siquiera tiene columnas para el desplazamiento del índice de páginas.

**Pero el índice de páginas no es lo que poda aquí.** Quien selecciona teselas son las cajas `x`/`y`
de las **estadísticas de row group**, que están en el footer y **las escriben los dos**. El índice de
páginas serviría para podar *dentro* de un row group de 4.096 filas, que es la unidad indivisible que
ADR-0042 §3.1 fijó: no hay nada que podar dentro. Son **179 kB que el lector nunca lee**.

### 4. Lo que no estaba en la pregunta: arrow-rs pierde un 25% de bytes por su propio defecto

| disposición | `cluster_id` | `dense_id` | `x` | `y` |
|---|---|---|---|---|
| arrow-rs, por defecto | 71,6 kB | 27,56 MB | 27,50 MB | 20,00 MB |
| arrow-rs, diccionario apagado | 20,03 MB | 20,03 MB | 20,03 MB | 20,03 MB |
| arrow-rs, diccionario por columna | 71,6 kB | 20,03 MB | 20,03 MB | 20,03 MB |
| `DuckDB` COPY | 79,5 kB | 20,04 MB | 20,04 MB | 20,04 MB |

`parquet` intenta `RLE_DICTIONARY` en todo. Sobre `dense_id` y `x` —casi únicos dentro de una tesela—
el diccionario más los índices cuestan **un 37% más que PLAIN**. Sobre `cluster_id` —un puñado de
valores repetidos— gana por 280×. `DuckDB` elige por columna y acierta las dos veces.

**Apagarlo entero es peor** (80,73 MB): destruye `cluster_id`. Apagarlo **por columna** deja arrow-rs
en 60,78 MB contra los 60,57 MB de `DuckDB` — empate, y 1,10 MB por ventana en los dos.

## Decisión

### 1. Un fichero con row groups de 4.096, no un fichero por tesela

Domina en las tres columnas medidas: **4× menos peticiones** (5,6 contra 22,3), **2,32× menos
footer** (496 kB contra 1,15 MB) y **2,0× menos metadatos** que el lector tiene que tener para poder
direccionar (676 kB contra 1,33 MB). Y no cuesta bytes: 75,81 MB contra 76,46 MB, donde la diferencia
son 1.220 footers que dejan de existir.

Y hay una consecuencia que la tabla no enseña: **A no tiene índice**. B paga una petición y se queda
el footer; A tendría que leer 1.221 footers, o escribir un sidecar que hoy no escribe nadie. ADR-0042
§3.3 lo resolvía con aritmética —«el lector computa todas las URL antes de emitir la primera»—, y eso
sigue siendo cierto para *qué teselas existen*; lo que la aritmética no da es **qué teselas
intersecan la ventana**, porque el rango de `dense_id` de una ventana no se computa sin conocer el
rango Morton, y el rango Morton es lo que las cajas dicen. **El footer es ese índice**, y es el mismo
footer que ADR-0042 §3.3 daba por eliminado.

Esto **revisa ADR-0042 §3.1 en un punto**: «una tesela es un rango fijo de 4.096 filas» se queda
entero; «una tesela es un fichero» nunca se decidió explícitamente y aquí se decide que no.

### 2. `batches_to_parquet` fija el tamaño de row group, y es lo único que fija

Hecho en este ADR: `crates/fossil-df/src/files.rs` pasa `max_row_group_row_count = DEFAULT_CHUNK_SIZE`.
El defecto de `parquet` es 1.048.576 filas, que mete cinco millones de vértices en **cinco** row
groups y deja el footer con cinco cajas: un lector direccionado no puede podar con eso. El test
`files::tests::a_row_group_is_a_tile` lo clava.

**Lo que no cambia aquí**: el emisor por teselas de `crates/fossil-runtime/src/layout.rs` sigue
escribiendo `chunk{k}.parquet` por tesela con `COPY`. Convertirlo es la otra mitad de esta decisión y
toca un crate que esta rama no tiene permiso para tocar.

### 3. `arrow-rs` en vez de `COPY` **no** está justificado por el índice de páginas

La justificación que el plan le daba —«`arrow-rs` escribe el índice de páginas por defecto; `DuckDB`
no y no piensa hacerlo»— es **cierta y no vale**: lo que poda son las cajas de row group, que
escriben los dos, y el índice de páginas son 179 kB que nadie lee porque no hay nada que podar dentro
de una tesela de 4.096 filas.

Lo que queda en pie a favor de arrow-rs es de otra clase, y es lo que hay que decir en voz alta:
**es el único escritor que existe en el navegador.** `fossil-df-wasm` no tiene `DuckDB`. Mantener dos
escritores con dos disposiciones distintas es lo que ADR-0046 §9 llamó «dos escritores mientras
`manifest.rs:13` afirma que sólo hay uno», y ése es el argumento entero.

Y si se adopta, **hay que apagar el diccionario por columna**, o el cambio cuesta un 25% de bytes
almacenados y un 25% de bytes por ventana contra el `COPY` que sustituye. Eso **no** se hace en este
ADR: `batches_to_parquet` es genérico sobre el esquema del batch y no sabe qué columna es geometría.
Decidirlo bien es darle al escritor la lista de columnas de dibujo, que es una decisión de contrato,
no de codificación.

## Consecuencias

**Lo que se vuelve más fácil.** El lector deja de necesitar un índice que nadie escribe: pide el
footer, tiene 1.221 cajas y computa el conjunto de row groups y sus rangos de bytes sin una segunda
petición. Y la coalescencia sale gratis — 22,3 teselas contiguas son 5,6 rangos.

**Lo que se paga.** Un fichero único es un objeto que se reescribe entero cuando cambia la
maquetación. Con teselas sueltas se podía reescribir una; con un fichero, no. Rehacer la maquetación
ya renumeraba todo (ADR-0042 §3.4), así que en la práctica el fichero se reescribía entero de todos
modos, pero deja de ser posible el caso que no se ha necesitado nunca.

**Lo que esto no prueba.**

- **Sin aristas.** Sólo vértices. La colocación CSR de ADR-0042 §3.2 no la toca esto: una tesela de
  aristas se clava por el rango de su origen en las dos disposiciones. Pero los números de peticiones
  y bytes de una ventana **real** son la suma de las dos mitades, y aquí sólo está medida una.
- **Sin red.** Una «petición» es un rango que un lector tendría que pedir. Ni latencia, ni reutilización
  de conexión, ni milisegundos. Los conteos son comparables con la tabla de ADR-0042 §3.1; los tiempos
  no se han medido.
- **La estructura de comunidades está estipulada.** No se corre `community_hierarchy`: son 2.048
  clusters de igual tamaño, colocados y renumerados por las mismas funciones que el escritor. Lo que
  gobierna la medición es el orden Morton sobre una maquetación grumosa, y eso sí se reproduce; una
  distribución de tamaños sesgada movería la columna `necesarias` y no está medida.
- **Sin compresión**, porque `batches_to_parquet` no la activa. Los bytes son techo; lo que sobrevive
  a encenderla son las razones entre disposiciones, no los megabytes.
- **Un solo tamaño de ventana y un solo zoom.** Nueve ventanas, cuadradas, dimensionadas sobre la
  central — que por eso dibuja 20.003 vértices y las otras 35.000. Las razones comparan; las columnas
  absolutas no.

**Y lo incómodo.** Las tres afirmaciones que F7 pedía comprobar salen: dos confirmadas y una
confirmada-pero-irrelevante. La cifra que estaba mal no era ninguna de ellas: era **el motivo**. El
plan justificaba cambiar de escritor por el índice de páginas, y el índice de páginas es lo único de
todo lo medido que ningún lector va a abrir.
