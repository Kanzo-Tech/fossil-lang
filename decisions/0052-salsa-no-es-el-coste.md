# ADR 0052: Salsa no es el coste, y el −52% nunca fue nuestro

**Date:** 2026-08-07
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Revisa ADR-0046 §6 y detiene F6 §§1–2. Las cifras son de este árbol, medidas el 2026-08-07
en release sobre Darwin 25.2 (Apple silicon), con `crates/fossil-mir/examples/query_time.rs` (dentro
del proceso) y `target/release/fossil` (el mandato entero). El recuento de ejecuciones es el de
`crates/fossil-mir/examples/query_executions.rs`, sin cambios. El −52,3% de Apollo se consultó el
2026-08-06 y sigue siendo suyo.

## Contexto

ADR-0046 §6 mantiene salsa y dice que el alcance está mal, con una medida y una cifra prestada. La
medida —503 ejecuciones a cien mappings, tres clavadas por fichero y cinco por mapping sin un solo
acierto— es correcta y se reproduce exactamente. La cifra prestada es el −52,3% que Apollo midió al
sacar salsa de `apollo-compiler`, y el propio ADR la marca como estimación. F6 pedía convertirla en
nuestra midiendo antes y después.

Se midió antes. **No hay después, porque el antes ya contesta la pregunta.**

### 1. Lo que cuesta la contabilidad de salsa

Una consulta `#[salsa::tracked]` que no computa nada, sobre claves internadas frías —o sea, internar
la clave, insertar el memo y anotar la arista de dependencia, y nada más— cuesta **72–76 ns** por
ejecución, estable entre 100 y 10.000 ejecuciones. Multiplicado por el recuento de ejecuciones del
camino por lotes, es un **techo** de lo que se puede recuperar quitándola: el trabajo del compilador
no cambia, y un núcleo sin salsa sigue teniendo que internar tipos y guardar resultados en algún
sitio.

| mappings | ejecuciones | compilación | techo del ahorro | |
|---|---|---|---|---|
| 1 | 8 | 32,9 µs | 592 ns | **1,8%** |
| 10 | 53 | 102,0 µs | 3,9 µs | **3,8%** |
| 100 | 503 | 1,537 ms | 37,2 µs | **2,4%** |
| 1.000 | 5.003 | 95,0 ms | 370,2 µs | **0,4%** |

**El techo es 3,8%**, y cae con el tamaño del programa. El −52,3% de Apollo no es alcanzable aquí ni
en el mejor caso imaginable.

### 2. Por qué la forma de Apollo no transfiere

`apollo-compiler` es una biblioteca cuyo trabajo entero es la compilación, así que el porcentaje de
salsa sobre la compilación *es* el porcentaje sobre el proceso. Aquí no. Medianas de 25 ejecuciones
del binario release, máquina en reposo:

| mandato | mediana | lo que añade respecto de la fila anterior |
|---|---|---|
| `fossil providers` | 8,74 ms | arrancar el proceso, y nada más |
| `fossil refs examples/hello.fossil` | 9,06 ms | **+0,3 ms** — leer, parsear, construir el `DefMap` |
| `fossil check` sobre un fichero **sin fuentes** | 14,19 ms | **+5,1 ms** — abrir una conexión DuckDB en memoria |
| `fossil check examples/hello.fossil` | 16,27 ms | **+2,1 ms** — un `DESCRIBE` de la fuente, más typecheck y lowering |
| `fossil check` sobre 100 mappings | 17,96 ms | **+1,7 ms** — noventa y nueve mappings más |

Esa última fila es la que cierra las dos mediciones: dentro del proceso, la compilación a cien
mappings mide **1,537 ms**, y el binario cobra **1,69 ms** por los mismos mappings. Miden lo mismo.

Y entonces la compilación completa de `hello.fossil` son **32,9 µs**: el **0,20%** del mandato.
Arrancar el proceso es el **54%**, y abrir DuckDB otro **31%** — nótese que el fichero sin fuentes ya
lo paga entero, así que lo caro es **abrir la conexión**, no describir.

Salsa, dentro de eso, es el 1,8% del 0,20%. **Es 0,004% del mandato**, y ninguna herramienta de
medición honesta lo distingue del ruido.

### 3. Y hay un coste real, que no es éste

La misma medición, partida por consulta, encuentra otra cosa. A cien mappings y a mil:

| consulta | 100 mappings | 1.000 mappings | crecimiento |
|---|---|---|---|
| `parse` | 161,1 µs | 1,479 ms | 9,2× |
| `def_map` | 15,3 µs | 55,4 µs | 3,6× |
| `mapping_cst_node` | 32,5 µs | 2,281 ms | **70×** |
| `body` | 69,0 µs | 680,4 µs | 9,9× |
| `typecheck_mapping` | 88,0 µs | 881,0 µs | 10,0× |
| `lower_to_mir_pg` | 1,142 ms | **89,2 ms** | **78×** |

Diez veces el programa, setenta y ocho veces el tiempo: `lower_to_mir_pg` es **cuadrático en el
número de mappings del fichero**, y a mil mappings es el **94%** de la compilación. La causa está en
el propio código, `crates/fossil-mir/src/lower.rs:166-177`: cada mapping construye la tabla de
esqueletos de sujeto de **todos** los mappings del fichero, y `subject_template_skeleton`
(`crates/fossil-mir/src/skeleton.rs:17`) no es una consulta memoizada, así que son n² recorridos con
una `String` nueva cada uno. `mapping_cst_node` tiene la misma forma más barata
(`filter(MAPPING).nth(idx)` sobre los hijos del fichero, `crates/fossil-hir/src/body.rs:164-169`).

Y lo incómodo: **salsa es lo único que impide que sea peor.** Las n² llamadas a
`subject_template_skeleton` hacen n² aciertos sobre `body`, que sin memoización serían n² lowerings
de cuerpo. Quitar salsa del camino por lotes sin arreglar antes esa tabla no ahorra un 3%: cambia el
factor del cuadrático a peor.

Ninguno de los dos importa hoy —un programa real tiene un puñado de mappings, y a cien la
compilación entera son 1,5 ms dentro de un mandato de 18—, pero es la única cifra de las tres tablas
que crece con el uso.

### 4. Lo que además bloquea F6 §§1–2, aunque mereciera la pena

`FossilDb` no es un detalle de implementación del camino por lotes: es el tipo por el que pasa toda
la API del núcleo. `def_map` devuelve un `DefMap<'db>` interned, `Ty<'db>` y `MappingLoc<'db>` son
`#[salsa::interned]`, los diagnósticos salen por un `salsa::Accumulator`, y `fossil_df::run_to_dir`
—el único camino de `run`— toma `&dyn fossil_base::Db`. Sustituir el `FossilDb` por «tres `OnceCell`
en un contexto» no es cambiar el sitio donde se construye la base: es reescribir la firma de
`fossil-hir` y `fossil-mir` enteros, unas 7.000 líneas, más `fossil-df`.

Y `fossil-df-wasm` no puede dejar de traer salsa aunque deje de construir la base: `cargo tree -p
fossil-df-wasm -i salsa` la alcanza por **`fossil-df`**, no por la dependencia directa de
`fossil-base`. El ejecutor mismo depende del núcleo tipado por salsa.

## Decisión

**1. Salsa se queda en el camino por lotes, y F6 §§1 y 2 no se ejecutan.** No por coste de
migración: porque el techo del ahorro es 3,8% de una compilación que es el 0,20% del mandato. ADR-0046
§6 diagnostica bien el alcance —la base se construye vacía en cada invocación y no hay
incrementalidad entre llamadas— y saca la conclusión equivocada de ello: que no haya aciertos entre
invocaciones significa que salsa no *gana* nada ahí, no que *cueste* algo que se note.

**2. El −52,3% de Apollo sale de nuestros documentos.** Era una cifra prestada, marcada como tal, y
ahora hay una propia que la contradice: **≤3,8%**, no 52%. La forma del argumento de Apollo —coste
fijo por consulta, domina cuando el trabajo por consulta es poco y el proceso es corto— es correcta;
lo que falla es la segunda mitad de la premisa. Nuestro proceso es corto *y el compilador no es lo
que lo llena*.

**3. Salsa sale de donde nunca se usó.** `fossil-runtime` la declaraba como dev-dependency y ningún
fichero suyo la nombra —su propio doc de módulo dice que el crate corre fuera del grafo de
consultas—. Se va, con los otros cuatro dev-deps muertos del mismo bloque (`fossil-mir`,
`fossil-descriptors-output`, `smol_str`, `tempfile`), que servían a un `tests/graphar_decomp.rs` que
ya no existe. Es la única parte de F6 §3 que era cierta.

**4. Lo que sí se persigue, cuando toque, es el cuadrático y el arranque.** En ese orden de tamaño:
8,7 ms de arranque de proceso y 5,1 ms de conexión DuckDB por `fossil check`, contra 32,9 µs de
compilación. Un `fossil check` que no abriera DuckDB cuando el programa no tiene fuentes que
describir se llevaría 5,1 ms él solo: **ciento cincuenta veces la compilación entera**, y unas ocho
mil veces el techo de quitar salsa. Esa función es de otro (`pre_introspect_and_register`), y este
documento sólo dice cuánto vale.

## Consecuencias

**Lo que se vuelve más fácil.** El objetivo de ADR-0046 §7 —cinco a ocho crates— deja de arrastrar
una reescritura de 7.000 líneas como precondición. Y el editor conserva sin discusión la única cosa
por la que salsa se adoptó: `crates/fossil-hir/tests/invalidation_regression.rs` sigue siendo la
prueba de que una edición de cuerpo no reejecuta el fichero.

**Lo que se vuelve más difícil.** Nada mecánico. Lo que se pierde es la narrativa: «quitamos salsa»
era una historia limpia, y «salsa no era el problema» obliga a decir cuál es. Está en §3 y §4, con
números.

**Lo que se paga.** `fossil-df-wasm` sigue enviando salsa al navegador dentro del artefacto con
`opt-level = "z"`. Este documento **no** mide ese coste: cuenta milisegundos nativos, no bytes de
wasm. Si el argumento del tamaño se quiere sostener, hay que medirlo aparte —y sería un argumento
distinto de éste, que trata del tiempo.

**El riesgo.** Las tres tablas salen de una máquina, una arquitectura y un programa sintético de
forma única (`n` mappings sobre una fuente). El cuadrático de §3 es una propiedad del código y se lee
en la fuente, así que no depende de la máquina; los porcentajes de §1 y §2 sí. Lo que los reabriría
es un perfil donde la compilación deje de ser microsegundos —un fichero con cientos de mappings, o un
`fossil check` que se llame en bucle dentro de un proceso vivo—, y lo primero está medido arriba: a
mil mappings el techo de salsa **baja** al 0,4%.
