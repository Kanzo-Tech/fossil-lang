# ADR 0051: La dirección base sobrevive al parser y muere en el pivote, y lo dice

**Date:** 2026-08-07
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Ejecuta la parte de la pista paralela de RDF 1.2 de ADR-0046 §8 que no depende de F2 ni de
F7. Las dos mediciones de `oxttl` son de este documento, tomadas el 2026-08-07 contra `oxttl` 0.2.3 /
`oxrdf` 0.3.3. El modelo abstracto es RDF 1.2 Concepts (CR de 2026-04-07); Turtle 1.2 sigue en
Working Draft, el cuarto desde mayo.

## Contexto

La pista de RDF 1.2 tenía dos cosas que se podían hacer sin nada más: **encender la feature `rdf-12`
de `oxttl`, que está apagada**, y **llevar la dirección base en los literales**. Las dos resultaron
ser distintas de lo que se creía, y en direcciones opuestas.

### La feature ya estaba encendida, y por accidente

Medido fuera de este workspace, con `oxttl` 0.2 en sus features por defecto:

| entrada | `rdf-12` apagada | `rdf-12` encendida |
|---|---|---|
| `ex:a ex:reifies <<( ex:s ex:p ex:o )>> .` | ``«`<<(` is not a valid RDF object»`` | 1 triple |
| `ex:a ex:label "hello"@en--ltr .` | «Literal base direction are only allowed in RDF 1.2» | 1 triple |
| `ex:a ex:label "hello"@en .` | 1 triple | 1 triple |

Y medido **dentro** del workspace, con `cargo tree -e features`: `oxrdf/rdf-12` y `oxttl/rdf-12`
están activadas por `rudof_iri` y `rudof_rdf`, que entran por `shex_ast` desde `fossil-hir` y
`fossil-shex`. Nunca las declaramos nosotros. Por eso el brazo `Term::Triple` de
`crates/fossil-df/src/rdf.rs` **compila hoy**: esa variante sólo existe con la feature, y la teníamos
sin pedirla.

Eso no es un detalle de empaquetado. Significa que **nuestro nivel de RDF era una propiedad del grafo
de dependencias de rudof**, no una decisión nuestra: el día que `fossil-hir` deje de depender de
`shex_ast` —que es exactamente lo que ADR-0046 §8 contempla al mover el tipo de destino a SHACL— el
parser vuelve a ser de RDF 1.1 y ese brazo se rompe **en compilación**, por una razón que nadie
conectaría con haber tocado el descriptor.

La feature no arrastra ninguna dependencia nueva: `oxttl/rdf-12 = ["oxrdf/rdf-12"]` y
`oxrdf/rdf-12 = []`. El cierre de wasm no cambia.

### La dirección base sobrevive al parser y no cabe en la columna

`"مرحبا"@ar--rtl` llega entero: `Literal::direction() == Rtl`, `language() == "ar"`, y el datatype es
`rdf:dirLangString` —disjunto de `rdf:langString`, como manda la spec—. Donde se pierde es **una capa
más abajo**, en `rdf_to_batch`: el pivote produce una celda `Utf8` por (sujeto, predicado), y una
celda `Utf8` lleva la forma léxica y nada más. Ahí ya se pierde también la etiqueta de idioma, que es
de RDF 1.1.

Y no es una pérdida cómoda. **La dirección base existe porque el primer carácter fuerte no determina
la maquetación**: ese es el caso de uso entero del tipo. Devolver `"مرحبا"` sin la dirección no da un
valor empobrecido, da **un valor que el host coloca al revés**, y no hay nada en la celda que permita
detectarlo aguas abajo.

Llevarla de verdad pide un tipo de columna que la lleve, y el tipo de columna es
`fossil_graph_schema::Primitive`, el único retículo del compilador desde ADR-0047. Añadirle
`dirLangString` hoy sería la equivocación que ADR-0046 diagnostica nueve veces: **la abstracción
antes de la segunda implementación**. No hay productor que emita uno —el perfil `1.2-basic` describe
un escritor, y no tenemos escritor de Turtle—, no hay descriptor que lo declare, y el retículo se
usa en crates que tendrían que abrir un brazo para un valor que nadie construye.

## Decisión

**1. `rdf-12` se declara en `[workspace.dependencies]`, sobre `oxttl` y `oxrdf`.** Es un no-op para
el grafo de hoy y la garantía para el de mañana. El comentario que la acompaña lleva la medición, no
la intención.

**2. La dirección base no se descarta en silencio: `rdf_to_batch` la rechaza nombrándola.** El error
es un `NotImplemented` que dice sujeto, predicado y literal. Un `rdf:langString` sin dirección sigue
pasando intacto: los dos tipos son disjuntos y rechazar uno no puede costar el otro.

**3. Aquí paramos.** El triple term como primitiva necesita F2 (el precipicio de expresiones) y el
almacenamiento con discriminador necesita F7; ADR-0046 §8 ya lo dice. Lo que este ADR añade es que
**la dirección base tampoco era gratis**, y por qué: no le faltaba el parser, le falta el tipo de
columna.

## Consecuencias

**Lo que se gana.** El nivel de RDF del árbol es una línea del manifiesto con su medición al lado, y
deja de ser un efecto secundario de rudof. Un `.ttl` con direcciones falla con un mensaje que dice
qué fila y qué predicado, en vez de producir texto invertido. Y los dos tests de
`crates/fossil-df/src/rdf.rs` son un guardián real por una vía barata: si la feature se apaga, **no
compilan** —`Term::Triple` y `Literal::direction` desaparecen del API—, así que el fallo llega antes
que cualquier aserción.

**Lo que se pierde.** Un corpus RDF 1.2 con literales direccionales deja de leerse entero, donde
antes se leía mal. Es deliberado y el radio es cero: nada se ha publicado y ningún fixture del árbol
tiene direcciones.

**Lo que queda abierto, y lo que lo reabriría.** Que `Primitive` gane `dirLangString` en cuanto
exista **un productor o un descriptor que lo pida** — un escritor de Turtle con perfil `1.2-basic`,
o una forma SHACL que declare el datatype. Ese es el disparador; hasta entonces el retículo no lo
necesita. Y la afirmación de la página de docs de que «hoy nuestro parser de Turtle rechazaría un
`<<(`» es falsa desde antes de este ADR: era verdad de `oxttl` por defecto, no del árbol.
