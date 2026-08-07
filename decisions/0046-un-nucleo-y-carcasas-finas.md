# ADR 0046: Un núcleo y carcasas finas, y el precipicio que hay debajo

**Date:** 2026-08-06
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Retira ADR-0044 entero y revisa ADR-0045 §7. Las mediciones del lenguaje son de este
documento; las del corpus están en `kanzo-ui/BENCHMARKS.md`. Las referencias externas —Gleam, Gluon,
rust-analyzer, Apollo, ty, Biome, rustc, DuckDB, PMTiles, Iceberg, Delta, Lance, Malloy, MillenniumDB
y las specs de RDF 1.2 y SHACL 1.2— se consultaron el 2026-08-06.

## Contexto

Ocho preguntas sobre por qué la arquitectura no se entiende produjeron tres auditorías del árbol y
cuatro de sistemas de referencia. Coinciden en un diagnóstico, y no es el que se buscaba.

**No es que el diseño sea malo: es que casi nunca se construyó.** Todos los hallazgos tienen la misma
forma — un mecanismo declarado y nunca cableado:

| declarado | construido |
|---|---|
| cuatro traits de extensión | una impl útil y **cero despachos dinámicos** cada uno |
| catorce operadores MIR | el lowering emite **cuatro** → las diez reglas de reescritura son inalcanzables |
| 56 funciones de stdlib | **no llegan al HIR** |
| `content_hash`, el token que invalida lo más caro del compilador | `String::new()` en todos los sitios de producción |
| `register_inferred_descriptor` | `panic!` por defecto |
| un `pub trait Sink` publicado en `architecture.md` | **no existe** |

Un solo error a nueve escalas: **la abstracción se escribió antes de que existiera la segunda
implementación.** Y uno que lo agrava: se documentó como terminada, así que dejó de poder
distinguirse la intención del estado — catorce afirmaciones falsas en los documentos de la raíz.

### La causa raíz, medida

`crates/fossil-hir/src/lower.rs:60` — `HirExpr` tiene **cuatro** variantes: `Template`, `FieldRef`,
`StringLit`, `PrefixedName`. El parser produce `BINARY_EXPR`, `UNARY_EXPR`, `TERNARY_EXPR`,
`PARTIAL_EXPR`, `ARG_LIST`, `RECORD_LITERAL`, `TRIPLE_TERM` y ocho más. Y un `grep` de esas clases de
nodo sobre `crates/fossil-hir/` **devuelve cero**: el lowering lee cuatro y descarta el resto.

**Y no falla ruidosamente.** Medido el 2026-08-06 con el binario del árbol sobre
`examples/users.csv`:

| programa | `fossil check` | `fossil run` | columna emitida |
|---|---|---|---|
| `ex:name = .name` | ok | *wrote 1 vertex type* | **`name`** |
| `ex:slug = clean.slug(.name)` | **ok** | *wrote 1 vertex type* | **ninguna** |
| `ex:doble = .id * 2` | **ok** | *wrote 1 vertex type* | **ninguna** |

**El compilador declara éxito y emite un corpus al que le falta la propiedad, sin un diagnóstico.**
Es pérdida de datos silenciosa en el punto ciego del sistema de tipos, y es el peor modo de fallo
posible para una herramienta cuyo principio rector es «si compila, corre».

### Y los anillos de ADR-0044 no los tiene nadie

ADR-0044 partió el árbol por **destino de despliegue**. Ninguna de las tres referencias lo hace:
Gleam corta por **IO/pureza** —*«compiler-core… is entirely pure and has no IO so that is provided by
the other Rust crates that wrap this one»*—, Gluon por **fase del compilador**, y rust-analyzer por
**riqueza semántica**, con tres *API Boundaries* declaradas.

Las tres cortan por **qué se le permite saber a un trozo de código**, y lo que prohíben es siempre lo
mismo: conocimiento del mundo exterior. Y cuando el corte es correcto **las carcasas se colapsan**:
el wasm entero de Gleam son 352 líneas, su binario 6, su transporte LSP 69; el CLI de rust-analyzer
ni siquiera es un crate.

Un corte por anillos invierte eso: hace del destino el eje primario, y entonces hay que decidir a qué
anillo pertenece una *función* como el LSP — y no hay buena respuesta. Gleam lo mete bajo el CLI,
rust-analyzer dentro del binario, Gluon en otro repositorio.

**Y ninguna de las tres comprueba su arquitectura con un test.** La regla más citada de
rust-analyzer la sostiene un comentario en un `Cargo.toml`, repetido literal en dos ficheros. Lo que
hace el trabajo es un documento que dice la invariante **y su razón**.

## Decisión

### 1. Fossil es RML con tipos, más una biblioteca de transformación de valores

Lo que RML no puede hacer: comprobar **antes** de generar. Lo que dbt no hace: identidad de sujeto,
descomposición vértice/arista, conformidad de forma, cardinalidad y RDF 1.2.

El material ya está y no se puede invocar: `crates/fossil-registry/src/lib.rs:616` en adelante tiene
`clean.{trim,lower,upper,slug,normalize_unicode,strip_html}` y `anon.{hash,hmac,redact}`.

### 2. Se cierra el precipicio CST→HIR. El HIR tiene ocho formas

Las cuatro de hoy más **llamada** (para `clean.*`/`anon.*`), **comparación**, **condicional** y
**operadores de pipeline**. Ocho, no catorce: cada una justificada por algo que el lenguaje tiene que
expresar. Lo que el HIR no necesite **sale de la gramática** — un token que no llega al HIR es una
promesa que el lenguaje no cumple.

**Y mientras tanto, descartar en silencio deja de ser aceptable:** una expresión que el lowering no
sabe bajar es un diagnóstico, no un `continue`.

### 3. El trabajo relacional vive en el lenguaje; el plan es de DataFusion

El operador `|>` compone fuentes y **nuestro checker lo tipa de punta a punta**. El pipeline tipado
baja a un `LogicalPlan`, que ya tiene empuje de predicados, reordenación de joins y coste.

Dos consecuencias que hay que leer juntas:

- **Los catorce operadores del MIR se quedan.** Son el vocabulario del pipeline, no una aspiración.
- **Las diez reglas de reescritura se van.** Y **no es renunciar a optimizar: es dejar de optimizar
  dos veces.** `rewrite()` no estaba incompleto — competía con un optimizador que ya está debajo, y
  perdía por definición.

### 4. La forma de salida es un contrato, y la sensibilidad es un tipo

La forma es el **tipo de destino** y el checker prueba que ningún mapping puede violarla. Es el
principio D9 hecho verdad; hoy el único descriptor que llega al typecheck trackeado es
`ACCEPT_ALL_DEFAULT` (`crates/fossil-hir/src/check.rs:96`), así que ese eje tiene un solo valor.

**Y la forma puede declarar que una propiedad es sensible.** Entonces un valor crudo no es asignable:
`anon.hmac(…)` devuelve un **seudónimo** —determinista y con clave, para que los joins sobrevivan— y
el seudónimo es lo único que encaja ahí. **La anonimización deja de ser una función que se te puede
olvidar y pasa a ser algo que el compilador no te deja saltarte.**

Eso es lo que hace que el sistema de tipos pague su coste, y es lo que ningún lenguaje de mapeo
ofrece.

### 5. La extensión es una tabla de `fn`, nunca un objeto de trait

Nadie mete la extensión dentro del núcleo memoizado. `rustc` —el abuelo de Salsa— usa un `Providers`
de punteros a función planos, ensamblado una vez con un `provide()` por crate, **ambiente en el
contexto y nunca parte de la clave de una consulta**. Rust-analyzer despacha ~250 assists con una
tabla `&'static` desde una función que **no** es tracked. DuckDB eligió esta forma para escapar de la
vtable de C++, y es literalmente el único eje abierto que fossil tiene hoy.

Quien usa objetos de trait es DataFusion, que **no tiene núcleo incremental** y tuvo que construir a
mano `DynEq`/`DynHash` sobre `Any` para recuperar la igualdad que un `fn` da gratis.

Si una entrada tiene que nombrarse *dentro* de una consulta, se nombra con un `&'static` cuyo
`Eq`/`Hash` sean `ptr::eq`/`ptr::hash`.

**No habrá plugins wasm ni dylib.** Zed mantiene diez versiones de ABI simultáneas; DuckDB una matriz
de seis plataformas por versión, firmada; SWC pagó la apertura con variantes `Unknown` en cada enum
de su AST. Una lista `provide()` en el árbol da el beneficio entero a coste cero de ABI, y sigue
siendo una puerta que se puede abrir después.

### 6. Salsa se queda, con el alcance en los crates del editor

La banda viva es 0.26–0.28 y estamos dentro. **Seis de las diez rarezas que produce aquí son su forma
normal** — aparecen literales en `ty` y en Mun.

Lo que está mal es el alcance: `crates/fossil-engine/src/system.rs:60-65` construye un `FossilDb`
nuevo con `Storage` vacío en **cada** llamada, y `crates/fossil-df-wasm/src/lib.rs:179` —en el
artefacto con `opt-level = "z"`— lo usa sólo para tener dónde alojar una consulta.

Apollo quitó Salsa de `apollo-compiler` en 2024: **−19,4% en la entrada grande y −52,3% en la
pequeña**, con *«We were not taking advantage of caching it provides»* como razón. La forma de esos
dos números es la que nos aplica: el coste por consulta es fijo, así que domina cuando el trabajo por
consulta es poco y el proceso corto. **Con programas pequeños, el −52% es nuestro perfil.**

**Y la medición ya está tomada** (`crates/fossil-mir/examples/query_executions.rs`, 2026-08-06). Se
cuenta `WillExecute` por consulta sobre la forma exacta del camino por lotes —un `def_map` y un
`lower_to_mir_pg` por mapping— a uno, diez y cien mappings. Salen **8, 53 y 503** ejecuciones, y se
parten en dos grupos limpios:

| consulta | a 100 mappings | qué significa |
|---|---|---|
| `parse`, `def_map`, `lower_to_hir` | **1 cada una** | clavadas por fichero: se computan una vez y las reutilizan los cien mappings |
| `body`, `mapping_cst_node`, `spans`, `typecheck_mapping`, `lower_to_mir_pg` | **exactamente 1 por mapping** | clavadas por mapping: **nada se reutiliza** |

**Lo que Salsa compra en el camino por lotes son tres entradas clavadas por fichero.** No es
despreciable —sin ellas el fichero se re-parsearía una vez por mapping, cien veces a cien mappings—
pero es literalmente lo que dan **tres `OnceCell`** en un struct de contexto. Las cinco consultas por
mapping ejecutan una vez cada una y no ganan nada: pagan la contabilidad de Salsa a cambio de cero
aciertos.

Así que la decisión se puede tomar sin más discusión: **el camino por lotes no necesita Salsa, y su
memoización cabe en una tabla.** Lo que la medición **no** dice es cuánto se gana quitándola — cuenta
ejecuciones, no milisegundos —, y esa cifra sólo la tiene Apollo. La forma de sus dos números sigue
siendo la mejor estimación que hay.

### 7. Un núcleo y carcasas finas. ADR-0044 se retira

El corte es por **lo que a un trozo de código se le permite saber**, no por dónde corre. **Objetivo:
cinco a ocho crates** para 45.000 líneas; hoy son veinticuatro.

Lo que sobrevive de ADR-0044 no es un anillo: son dos crates con un `cfg` y una razón escrita —
`crates/fossil-lsp/src/lib.rs:14` y `crates/fossil-resolver/src/lib.rs:50`.

Y una regla que se escribe **y además** se comprueba, en ese orden de importancia: `duckdb` y
`datafusion` en un crate cada uno, ninguno alcanzable desde una carcasa wasm.

**`lsp-types` en exactamente un `Cargo.toml`.** Es la invariante que rust-analyzer defiende con más
fuerza y cuesta 3.180 líneas de `to_proto.rs` — el 0,8% de su código. Hoy `fossil-ide` devuelve
`lsp_types::CompletionItem` directamente (`crates/fossil-ide/src/completion.rs:69`): hicimos el fallo
de Gleam, no la elección de rust-analyzer. Y los cuatro tipos de `fossil-run-status` son la mitad de
salida de un protocolo, que en rust-analyzer viviría en el crate binario.

### 8. RDF 1.2: una forma de expresión nueva y un tipo nuevo

**Nada de RDF 1.2 es Recomendación.** Concepts y Semantics están en CR (2026-04-07); **todas las
sintaxis concretas siguen en Working Draft**. Apuntamos al **modelo abstracto**, que es lo estable, y
tratamos la sintaxis como blanco móvil.

Tres cosas del modelo gobiernan el diseño: un triple term **sólo va en posición de objeto**; **no
tiene identidad propia** —quien la lleva es el **reificador**, y la spec dice que *«it is expected
that the reifiers (rather than the triple terms) will be used in further statements»*—; y
`rdf:reifies` es el predicado mediador, con rango `rdfs:Proposition`.

Nuestro `<<…>>` (`grammar.bnf:241`) es de la era RDF-star, que divergió en cuatro puntos.

**La primitiva es una:**

    <<( sujeto predicado objeto )>>   ::  Statement

Con dos reglas **de tipado, no de gramática**: un `Statement` sólo es asignable donde la forma
declara rango `rdfs:Proposition`; y los tres componentes tienen que ser evaluables sobre la fila
actual. La segunda es una regla que RML no puede expresar.

**Y un reificador no necesita sintaxis: ya es un mapping** — un IRI con propiedades. El azúcar copia
la jerarquía de la spec (un bloque indentado más el operador `~`, libre en nuestro léxico) y
**desazucara a un mapping**, así que el IR no gana nada.

**El tipo de destino es SHACL, no ShEx.** SHACL 1.2 Core (WD 2026-08-03) tiene `sh:TripleTerm` como
node kind y un `ReifierShapeConstraintComponent` con `sh:reifierShape`, y **constriñe sobre el
reificador**. ShEx no tiene grupo de trabajo W3C ni menciona RDF 1.2. *Esta última comprobación está
verificada a medias y hay que rehacerla antes de actuar.* Hoy la inversión está al revés:
`fossil-shex` es el único descriptor real y la variante `Shacl` es la que nadie construye.

### 9. El almacenamiento de una sentencia, y el escritor de Parquet

**No hay precedente**: ningún formato columnar de grafos admite que una arista sea extremo de otra —
ISO GQL y openCypher definen `N ∩ E = ∅`, así que es imposible por definición.

Como RDF 1.2 quitó la posición de sujeto, **nunca necesitamos un id de arista como sujeto**. Y el
coste se parte en dos, predecible: **anotar una relación es gratis** (la sentencia ya es una fila de
la tabla de aristas y su identidad es su posición); **anotar un dato cuesta una fila** (es una
columna, y no hay fila a la que apuntar). Eso obliga a un **discriminador en el espacio de ids** — el
modelo de MillenniumDB — y el ancho ya está pagado desde `e385617`.

Tres hechos medidos que lo fijan: la **propiedad singleton queda descartada** (un predicado por
sentencia destruye el diccionario y el RLE, y rompió cuatro de cinco motores); **se presupuesta en
bytes, no en triples** (RDF-star recortó los triples un 70% y el disco creció 3,5×); y **nunca se
clava un triple term por su `(s,p,o)`**, porque dos aristas paralelas con procedencias distintas
colapsan en una. La mitigación es la de la propia spec: varios reificadores sobre un mismo triple
term, y un reificador es un vértice.

**Y el escritor cambia.** Hoy **no hay una sola opción de Parquet configurada en el workspace** y hay
**dos escritores** mientras `crates/fossil-sinks/src/manifest.rs:13` afirma que sólo hay uno. DuckDB
no escribe el índice de páginas y no piensa hacerlo, y su umbral de corte son 100 MiB, así que su
granularidad de página **es** la del column chunk. `arrow-rs` lo escribe por defecto y **ya está en
el árbol vía DataFusion**.

Medido el 2026-08-06 sobre el corpus de un millón: una tesela de 4.096 filas **es un row group**, su
footer son 710 bytes, y las estadísticas traen min/max de `x` e `y`. Nueve ventanas: las cajas
seleccionan **14–23 teselas donde hacen falta 13–21**, sobrelectura **1,05×–1,21×**. **El índice ya
está en el fichero.** Si el fetch usa esas cajas, el lector **nunca calcula un Morton** y la
obligación de publicar la aritmética se reduce al escritor.

## Consecuencias

**Lo que se vuelve más fácil.** Cerrar el precipicio hace alcanzables las 56 funciones, convierte al
registry en punto de extensión real, y obliga a que `Primitive` baje a un crate hoja — con lo que
mueren la cadena de `InferredColumn.primitive` y seis de sus siete tablas duplicadas.

**Lo que se vuelve más difícil.** El pipeline propio nos pone a competir con SQL en su terreno: hay
que tipar `join` y `where` de verdad, y el checker pasa a ser el trabajo principal, no un accesorio.

**Lo que se paga.** El book no puede enseñar el stdlib hasta que §2 aterrice, y `stdlib.md` describe
582 líneas de funciones inalcanzables. Documentarlas antes sería documentar una pérdida silenciosa
como si fuera una funcionalidad.

**El riesgo mayor, dicho antes de cobrarlo.** RDF 1.2 no está ratificado y su sintaxis lleva cuatro
borradores desde mayo. Diseñar contra el modelo abstracto lo acota, pero si la sintaxis se mueve otra
vez, se mueve nuestro azúcar.

**Y lo incómodo.** Este documento retira un ADR de hace un día y revisa otro de hace dos. Los tres
salieron de la misma sesión, y la diferencia es que éste midió antes de decidir: la tabla de la
pérdida silenciosa no existía cuando se escribieron ADR-0044 y ADR-0045, y es lo que reordena la
prioridad. **Cerrar la frontera CST→HIR va antes que cualquier movimiento de crates**, porque mover
crates alrededor de un compilador que pierde propiedades en silencio es ordenar las sillas.
