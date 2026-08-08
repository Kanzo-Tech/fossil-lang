# ADR 0055: La forma es ShEx, y el checker la lee por el mismo camino que la fuente

**Date:** 2026-08-07
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Desbloquea F4 de ADR-0046, que se declaró bloqueada por «la comprobación de ShEx, que está
a medias y decide si esto es SHACL». Contradice en parte
`apps/docs/content/docs/characteristics/types.mdx:61` («the shape language is SHACL, not ShEx») y
cierra el punto abierto de `rdf12.mdx:156`. El estado de SHACL 1.2 Core se comprobó contra el WD del
2026-08-03; el de ShEx, contra el Final Community Group Report de 2019.

## Contexto

### Lo que el comprobador de formas ya hace, y por qué no se nota

La comprobación hacia atrás **está escrita**. `resolve_target_shape`
(`crates/fossil-hir/src/shapes.rs:196`) convierte un `ShapeBinding` de ShEx en una tabla de
constraints tipadas; `check_property` (`crates/fossil-hir/src/check.rs:581`) casa el IRI del
predicado contra esa tabla y llama a `compatible` (`check.rs:200`), que corre las cinco reglas de
subtipado y culpa a dos spans. Sus tests pasan.

No corre nunca. `typecheck_mapping` es `#[salsa::tracked]` sobre `&dyn fossil_base::Db`, y la
vtable fina —por ADR-0006— no alcanza `OutputDescriptorKind`. Así que la única llamada de producción
pasa el literal (`check.rs:96-99`) y `resolved_shape` es `None` en todos los mappings del mundo. De
las cuatro cosas que `types.mdx` promete, **tres son código inalcanzable y la cuarta no existe**:
nadie lee `ShapeBinding.closed` (`crates/fossil-shex/src/lib.rs:70`, poblado en `:541`), que es el
único bit del que puede salir «propiedad no declarada».

El descriptor **sí** llega al ejecutor: `fossil-engine` lo resuelve (`src/lib.rs:302`) y
`apply_output_shape` (`crates/fossil-mir/src/lower.rs:346`) clasifica aristas contra él. Hoy la forma
decide **qué se escribe** y no decide **qué es un error**, que es el reparto exactamente al revés.

### Tres lenguas en el árbol, y sólo una es un contrato

- **ShEx** — `crates/fossil-shex` (1.196 líneas nuestras sobre el AST de rudof): parsea ShExJ y
  ShExC, resuelve `TripleExpr::Ref` con detección de ciclos, rechaza `OneOf` con una sugerencia de
  troceado, y decodifica la cardinalidad completa (`Exact/ZeroOrOne/OneOrMore/ZeroOrMore/Range`,
  `lib.rs:96`).
- **SHACL** — `crates/fossil-df/src/shacl.rs`, 236 líneas **nuestras**, no de rudof: un paseo por el
  grafo de formas con `oxttl` que proyecta once constantes del vocabulario Core sobre `GraphSchema`.
  Se alcanza sólo desde `fossil-df-wasm::build_descriptor` (`src/lib.rs:192`) —el playground—, nunca
  desde el motor nativo, y `resolve_target_shape` lo cortocircuita como si fuese `AcceptAll`
  (`shapes.rs:207`).
- **`GraphSchema`** — `crates/fossil-graph-schema`, la hoja sin dependencias donde ADR-0047 bajó el
  retículo. Es el modelo canónico al que **las dos** lenguas bajan (`to_graph_schema`).

### Lo que ShEx ya escribe en un programa fossil

El descriptor de **entrada** habla ShEx y sólo ShEx: `inferred_descriptor_from_shex`
(`crates/fossil-descriptors-input/src/shex.rs:43`). `def_map` lee un `.shex` desde el sistema de
ficheros, dentro de una consulta trackeada, para resolver los miembros de un destructuring
(`crates/fossil-hir/src/def_map.rs:437`). Y `resolve_source_row` —el ayudante llano que
`typecheck_mapping` ya llama— **lee, parsea y tipa desde un `.shex`** en el mismo cuerpo del
comprobador (`crates/fossil-hir/src/infer.rs:167-190`). El motor nativo cierra el círculo tomando ese
mismo argumento `schema =` de `io.rdf` como forma de salida (`fossil-engine/src/lib.rs:307-341`): un
programa con fuente RDF lleva **un** documento que es a la vez esquema de entrada y contrato de
salida.

Los tres ejemplos publicados (`packages/examples/src/{hello,ecommerce,musicbrainz}`) llevan `.shex`.
No hay un solo fichero SHACL en el árbol fuera del literal de test de `shacl.rs:266`.

### El argumento de RDF 1.2, comprobado

`types.mdx:61` elige SHACL porque «SHACL 1.2 Core carries `sh:TripleTerm` as a node kind and a
reifier-shape constraint component». **Es cierto**: `sh:TripleTerm` es una de las siete instancias de
`sh:NodeKind`, y `sh:reifierShape`/`sh:reificationRequired` son el componente §7.8.5. ShEx no tiene
nada de eso, y su especificación es un Final Community Group Report de 2019 (la versión nueva se
define en un grupo de trabajo del IEEE, no del W3C), frente a un Data Shapes WG recarterado hasta
diciembre de 2026.

Pero el documento que lo lleva **es un Working Draft del 2026-08-03**, y el que es Recommendation
—SHACL 1.0, de 2017— no tiene ni el node kind ni el componente. `rdf12.mdx:14` fija la regla de la
casa para exactamente esta situación: *«el modelo es estable y la sintaxis es un objetivo móvil»*, se
diseña contra el modelo abstracto y no contra un borrador. Aplicada a sí misma, esa regla dice que
el argumento de RDF 1.2 **no vence todavía**, y que además no es el eje que F4 decide: ni la
cardinalidad ni la clausura son preguntas de RDF 1.2.

### Y una cosa que F4 §2 dice mal

«Propiedad no declarada → error» **no es cierto en ninguna de las dos lenguas**. Una forma de ShEx es
abierta salvo que diga `CLOSED`; una de SHACL, salvo `sh:closed true`. Escrito como está, F4 §2
convierte en error el caso normal del mundo abierto. El bit existe y está poblado en ShEx
(`ShapeBinding.closed`) y **no** está ni parseado en nuestro SHACL (`shacl.rs` no nombra `sh:closed`).

### El hueco que nadie ha nombrado

Un programa con fuente CSV **no tiene dónde escribir su forma de salida**. `schema =` en `io.csv`
significa el descriptor de ENTRADA (`def_map.rs:51`, CSVW, que ADR-0037 deprecó); el motor sólo mira
el `schema =` de las fuentes `Provider` (`lib.rs:310-317`). La cabecera del mapping nombra el IRI de
la forma (`User : ex:Person`) y nada nombra el documento. Así que **la prueba de terminación de F4
—`ex:edad = .nombre` contra una forma que declara `xsd:integer`— no se puede escribir hoy** con la
fuente que usan todos los ejemplos.

## Decisión

### 1. La lengua del contrato es ShEx

No por ser mejor estándar —no lo es, y en 2026 va perdiendo—, sino porque en este árbol es la única
que ya es un contrato: es lo que escribe el descriptor de entrada, lo que lee `def_map`, lo que lee
`resolve_source_row`, lo que resuelve el motor nativo, y lo único de lo que existe una tabla de
constraints. Elegir SHACL aquí obligaría a un programa a llevar **dos** documentos de forma en dos
lenguas para el mismo grafo, porque el lado de entrada no va a dejar de ser ShEx en F4.

### 2. SHACL se queda exactamente donde está, y eso es una decisión y no una omisión

`OutputDescriptorKind::Shacl(GraphSchema)` sigue existiendo, sigue alimentando al ejecutor por
`to_graph_schema`, y sigue sin llegar a `resolve_target_shape`. Nuestro SHACL **no puede** sostener
F4 §2 tal como está escrito: tira `sh:minCount` a propósito (`shacl.rs:17-19`), colapsa `sh:maxCount`
a un booleano (`shacl.rs:150-154`) y no lee `sh:closed`. Un contrato que no distingue «uno» de «uno o
más» no es el contrato que F4 pide.

### 3. El comprobador lee la tabla de ShEx, **no** `GraphSchema`

Es la tentación obvia —«comprueba contra el modelo canónico y valen las dos lenguas»— y no funciona:
`fossil_graph_schema::Cardinality` tiene dos variantes (`lib.rs:124`) y `NodeType` no tiene bit de
clausura. Comprobar contra `GraphSchema` es comprobar dos de los tres ejes de F4 §2 y perder los
otros. El modelo canónico se queda donde sirve: en el materializador.

### 4. El descriptor llega **leyendo**, no por una clave de Salsa nueva

`resolve_target_shape` pierde el parámetro `kind: &OutputDescriptorKind` y resuelve el documento de
forma por ruta, igual que hace su vecino `resolve_source_row` doce funciones más arriba en el mismo
cuerpo trackeado. `ACCEPT_ALL_DEFAULT` desaparece de `check.rs:96` **con el parámetro**, no
sustituido por otra cosa.

No hace falta ni el accesor nuevo en `System` (que sería un ciclo: `fossil-descriptors-output` depende
de `fossil-base`) ni ensanchar ninguna clave: leer por `System::read_file` desde dentro de una
consulta trackeada no registra dependencia —es la misma regla que ADR-0053 escribió para la caché— y
`MAX_PER_MAPPING_FAN_OUT` se queda en 1 sin tocar `invalidation_regression.rs`.

**Los saltos que faltan, en orden:**

1. **Dónde se declara la forma** (§5 abajo) — es lo único que pide gramática.
2. `fossil-hir/src/shapes.rs` — `resolve_target_shape` lee el documento por ruta y lo parsea con
   `ShExDescriptor::from_shex_source`, no `from_reader`.
3. `fossil-hir/src/shapes.rs:78` — `ResolvedShape::from_binding` copia `binding.closed`, que hoy
   descarta.
4. `fossil-hir/src/check.rs:96-99` — muere el literal.
5. `fossil-hir/src/check.rs:581` — `check_property` gana el brazo del predicado sin constraint.
6. `fossil-hir/src/check.rs:200` + `:442` — `compatible` gana el techo (`Seq` contra `max ≤ 1`), que
   hoy sólo mira el suelo (`Optional` contra `min ≥ 1`).
7. `fossil-hir/src/check.rs:116-121` — un paso nuevo **después** del bucle de propiedades: los
   constraints con `min ≥ 1` que ningún `HirProperty` escribió. No existe nada parecido hoy.
8. `fossil-engine/src/lib.rs:86` — `check()` no resuelve descriptor ninguno; con §4 deja de
   importarle, y ésa es la señal de que §4 es la forma correcta.

### 5. El documento de forma se declara una vez por programa, y hoy no hay dónde

> **Superado por ADR-0057 §2.** Este apartado da por supuesto que el documento es la autoridad y que
> sólo falta decir dónde está. ADR-0057 invierte eso: el documento es un ORIGEN de tipos, el tipo se
> puede escribir en el lenguaje, y entonces no hay nada que declarar. Lo que sigue se conserva porque
> el diagnóstico —un programa con fuente CSV no tiene dónde escribir su contrato— es el que llevó ahí.

El motor ya trata la forma como *program-resident* y rechaza dos formas distintas en un programa
(`lib.rs:322-328`). Lo que falta es el sitio para escribirla cuando la fuente no es RDF. **No se
recicla `schema =`**: significa ya el descriptor de entrada de esa fuente, y un argumento que
significa dos cosas según el constructor es la ambigüedad que ADR-0053 acaba de quitar de la caché.
Va al nivel del fichero, junto a los `prefix`. Es la única parte de esta ADR que toca la gramática y
la única que necesita una segunda firma.

### 6. «Propiedad no declarada» es un error **si y sólo si** la forma es cerrada

F4 §2 se corrige aquí. En una forma abierta, un predicado que la forma no declara es legal y sale del
comprobador sin tipo esperado —es lo que ya pasa, sólo que hoy pasa por accidente—. En una forma
`CLOSED`, es error y nombra la forma. Sin este bit, F4 §2 hace ilegal el mundo abierto, que es el
mundo por defecto de RDF.

### 7. Orden respecto a ADR-0054

ADR-0054 primero, F4 después, y no es preferencia: ADR-0054 §2 justifica el `Inner`-sólo **con** la
ausencia de F4. Los dos escriben en el cuerpo de `typecheck_mapping` —el join en la línea de
`resolve_source_row` (`check.rs:83`), F4 en la de `resolve_target_shape` (`check.rs:93-99`)— y los dos
tocan `compatible`, el join para exigir la misma `Primitive` en las dos `k` y F4 para la cardinalidad.
Lo demás es disjunto: `infer.rs` y `lookup_field` son sólo del join; `shapes.rs` y `check_property`
son sólo de F4.

## Consecuencias

**Lo que se vuelve más fácil.** El eje que `types.mdx` llama «job two» pasa de tener un valor a tener
los que la forma diga, y con él las tres comprobaciones que ya estaban escritas empiezan a correr sin
escribirse otra vez. `fossil check` deja de necesitar el descriptor resuelto por el host, que es la
razón por la que hoy `check` y `run` no ven lo mismo. Y F4 §3 —la sensibilidad como tipo— hereda un
sitio donde declararse: una anotación en la forma, que es donde `types.mdx:128` ya sospechaba que
estaba el problema.

**Lo que se paga, dicho en voz alta.** Se apuesta por una lengua cuyo grupo de estandarización
migró al IEEE, contra una que tiene WG del W3C hasta diciembre de 2026 y ya lleva el node kind que
RDF 1.2 necesita. La apuesta es reversible por construcción —el modelo canónico es `GraphSchema`, y
las dos lenguas bajan a él— pero la **tabla de constraints** que el comprobador lee es de ShEx, y ésa
sí habría que reescribir: `sh:minCount`/`sh:maxCount` a `Cardinality`, `sh:closed` al bit, y
`sh:reifierShape` a lo que RDF 1.2 acabe pidiendo. Estimado en el tamaño de `fossil-shex`, es la
mitad corta: el paseo por el grafo ya está escrito y lo que falta es el decodificador.

Se paga también que el motor nativo sólo lee ShExJ (`from_reader`, `fossil-engine/src/lib.rs:339`)
mientras el playground lee las dos sintaxis (`from_shex_source`, `fossil-df-wasm/src/lib.rs:203`).
Un consumidor de keasy escribe hoy el JSON a mano —los tres ejemplos son JSON— cuando la sintaxis
compacta ya está soportada en el árbol. Es una línea, y va en el salto 2.

**Lo que la reabre.** Tres cosas, y ninguna es un argumento:

1. **Un cliente que llegue con SHACL escrito.** No SHACL como preferencia: un fichero `.ttl` que ya
   existe y que alguien no va a reescribir.
2. **SHACL 1.2 Core llegando a Recommendation** con `sh:TripleTerm` dentro, a la vez que un productor
   real de RDF 1.2 en el corpus. Las dos cosas, no una: el node kind sin el productor no comprueba
   nada.
3. **Un `.shex` que necesite `OneOf`.** Hoy se rechaza con una sugerencia de troceado
   (`fossil-shex/src/lib.rs:624`), y esa sugerencia es la que va a sonar absurda el día que la forma
   la escriba un tercero y no nosotros.
