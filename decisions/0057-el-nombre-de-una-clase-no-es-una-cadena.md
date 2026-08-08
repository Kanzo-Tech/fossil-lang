# ADR 0057: El nombre de una clase no es una cadena, y un documento de formas es un origen de tipos

**Date:** 2026-08-08
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Rediseña la superficie que `type-system.md` §5 ya formaliza (`shape S ∈ Γ` — la forma **ya** es
una entrada del entorno) y que la sintaxis nunca dijo. Sustituye el §5 de ADR-0055 (`shape <...>` a
nivel de fichero). Se apoya en un estudio de 17 lenguajes de mapeo a RDF y 12 sistemas de
esquema-como-código hecho el 2026-08-07/08; las citas literales viven en ese estudio y aquí van sólo
las que deciden.

## Contexto

Hoy un mapeo se escribe así, y hay tres cosas mal en cinco líneas:

```
prefix ex: <https://example.org/>
users := io.csv("users.csv")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
```

**1. `ex:Person` es una cadena opaca.** Nadie comprueba que esa forma exista. Escribes `ex:Persn` y
sale RDF perfectamente válido con un IRI que no significa nada: sin error, sin aviso, y el fallo
aparece semanas después como un grafo con cero instancias. Y no es un descuido nuestro — **es el
estado del arte**. De 17 lenguajes de mapeo estudiados, 15 tratan el IRI de clase como una secuencia
de caracteres que se concatena y se emite. Los dos que comprueban algo llegaron por accidente: Ontop
porque necesitaba aridades para reescribir SPARQL→SQL (y aun así `checkClass` sólo lanza cuando el
término está declarado con **otro** rol: un IRI que no ha visto nunca pasa en silencio), y Karma
porque un desplegable de GUI no puede ofrecer lo que no existe.

Peor: **la dirección está invertida en todo el ecosistema.** RML2SHACL, Astrea, SCOOP y el propio
ShExML *derivan la forma del mapeo*. Nadie comprueba el mapeo contra la forma. Eso convierte la forma
en una descripción a posteriori de lo que el mapeo hizo, erratas incluidas. ShExML merece nombrarse
aparte porque es el que más se parece a nosotros y la trampa es fácil: toma prestada la sintaxis de
ShEx y **genera** el esquema; su `:Person` es una etiqueta de forma, no una clase, y no produce
`rdf:type`. No resuelve el problema, lo disfraza.

**2. `` `${ex:}user/${.id}` `` es un prefijo interpolado dentro de una cadena.** Es la línea más fea
del lenguaje, y el estudio dice exactamente por qué: hay dos familias de plantilla de sujeto, la de
cadena (`rr:template "http://…/{EMPNO}"`, donde ningún prefijo cabe) y la de **CURIE con agujeros**
(`:uni1/student/{s_id}` en Ontop, `s: ex:$(id)` en YARRRML, `:[films.id]` en ShExML). Estamos en la
primera por accidente. En la segunda, la línea es `iri = ex:user/{id}` y no hay nada que escapar.

**3. `prefix` hace dos trabajos.** Abrevia IRIs que escribes a mano, y sirve de base para acuñar los
sujetos. El segundo no es un prefijo: es una **base**, y D2RQ ya la tenía en 2004 con patrones
relativos, «*para mapeos portables entre instancias del mismo esquema*».

### Lo que el arte previo prohíbe, medido

- **La clausura no puede ser una bandera del emisor.** LinkML la saca de `--closed`/`--non-closed` en
  la CLI y TypeSpec de `seal-object-schemas`; son exactamente dos de los tres sistemas donde el
  round-trip es imposible por construcción, porque el mismo tipo emite dos documentos distintos.
- **No derives la identidad del nombre.** Es la trampa de LinkML (`slot_uri` omitido ⇒ IRI derivado
  del nombre del slot): renombrar cambia la identidad en silencio, que en un lenguaje que escribe
  grafos es una migración de datos disfrazada de refactor. Lo que mejor ha envejecido de todo el
  estudio es lo contrario: el número de campo de protobuf — identidad pequeña, permanente e **ajena
  al nombre**, con «*never reuse*» y `reserved` como lápida.
- **No pongas la identidad en una anotación.** GraphQL apostó por las directivas como mecanismo de
  extensión y publicó un transporte que no las lleva: las directivas aplicadas siguen sin ser
  introspectables cinco años después. Smithy tiene la versión suave: «*custom traits … are not
  converted and added to the OpenAPI specification*». **Lo extensible del lenguaje es justo lo que no
  sobrevive a la exportación.**
- **`EXTRA` de ShEx es un tercer estado**, no la negación de `CLOSED`: dice que un predicado puede
  repetirse con valores que no casan. Un booleano no lo representa — y `ShapeBinding.closed` es un
  booleano.
- **No tener espacios de nombres no simplifica el lenguaje**, lo muda a las herramientas de abajo:
  GraphQL es el resultado negativo limpio, y LinkML («*imports work like `import *`*») lleva desde
  2024 con la discusión abierta.

## Decisión

**1. El IRI de clase deja de ser una cadena opaca: se resuelve contra un proveedor de tipos.** Un
`use <personas.shex>` trae tipos al entorno, y `ex:Person from adultos` es un error de compilación si
esa forma no está entre los que trae. Es lo que `type-system.md` §5 ya escribía (`shape S ∈ Γ`) y lo
que ningún lenguaje de mapeo de fichero hace hoy.

**2. Un documento de formas es un ORIGEN de tipos, nunca una autoridad.** El tipo se puede escribir
en el lenguaje; el `.shex` es formato de importación **y de exportación**. Un programa sin documento
sigue siendo legal — declara sus tipos y `fossil` sabe emitir el `.shex` correspondiente. Esto
sustituye al §5 de ADR-0055: no hace falta declarar dónde está el documento porque el documento es
opcional por construcción.

**3. La plantilla de sujeto es un CURIE con agujeros, contra una base.** `iri = ex:user/{.id}`, y
`base <https://example.org/>` para la forma relativa `iri = /user/{.id}`. Muere `${ex:}` y muere el
backtick en posición de sujeto. `prefix` se queda para el vocabulario, que es su único trabajo real.

**4. Clausura, cardinalidad y `EXTRA` viven en el TIPO.** Nunca en una bandera del emisor, nunca en
una opción de CLI. Es la condición sin la cual el criterio de terminación de §7 no significa nada.

**5. La identidad tiene que ser un nodo del modelo, y jamás derivarse de un nombre.** *(Reformulado
el mismo día — ver la enmienda al final; la redacción original decía «los atributos nunca llevan
identidad», que es un criterio equivocado.)* Documentación y sensibilidad (F4 §3) son accesorias; el
IRI de clase y el de predicado son identidad, y por eso no pueden salir de una regla de casing sobre
el nombre del campo. Que se escriban como CURIE en línea (`ex:name = .name`) o como atributo del
modelo es elección de sintaxis, no de corrección.

**6. Nada se pierde en silencio.** Lo que la importación no sepa modelar entra como **nodo opaco que
nombra el constructo** (`Unsupported("OneOf")`, al estilo de `Unsupported("circle")` de Prisma) y
sale igual; lo irrepresentable al emitir se **rechaza por defecto** (zod `unrepresentable: "throw"`)
en vez de degradarse (el `skipped-choice` de pydantic, silenciado de fábrica). Hoy `fossil-shex`
rechaza `OneOf` con una sugerencia de trocear, y ADR-0055 ya sospechaba que esa sugerencia sonaría
absurda cuando la forma la escriba un tercero: un nodo opaco lo convierte en round-trip correcto sin
implementar el constructo.

**7. El criterio de terminación es un número, no una promesa.** El round-trip **textual** no lo cumple
nadie, y pedirlo es pedir un CST con trivia y medir comentarios. Lo que sí se cumple, y en nuestro
dominio además es estándar:

- **Solidez de importación**, la garantía de `Extract` de CUE: para todo grafo `g`,
  `valida(doc, g) == valida(importar(doc), g)`. Dice lo único que importa — que nuestro modelo de
  tipos **no miente** sobre lo que el documento aceptaba — y se comprueba por *fuzzing* de grafos.
  Ninguno de los doce sistemas estudiados lo automatiza.
- **Punto fijo de emisión módulo isomorfismo**: `parse(emit(T)) ≡ T`, y para el documento la
  equivalencia ya es Recommendation — **RDFC-1.0** (2024) hace decidible «el mismo grafo de formas».
  Es la forma GraphQL (idempotencia tras normalizar), no la identidad de bytes.
- **El corpus como porcentaje**: importar shexTest y exigir equivalencia al re-emitir. Los fallos
  *son* la lista de construcciones que al tipo le faltan. La dirección de escritura no parte de cero:
  `shex_compact` de rudof ya tiene printer además de parser.

**8. El nombre del mapeo desaparece.** Medido: `HirMapping.name` se lee en un sitio, para etiquetar un
diagnóstico. `ex:Person from adultos` es la comprensión de conjuntos que la cabecera siempre quiso
ser, y no hace falta flecha nueva — `<-` sería una segunda grafía de un ligado que ya tiene `:=`.

El programa completo:

```
base   <https://example.org/>
prefix ex: <https://example.org/voc#>
use    <personas.shex>

users   := io.csv("data/*.csv")
adultos := users |> where(.edad >= 18)

ex:Person from adultos
    iri = /user/{.id}
    ex:name = .name
```

## Consecuencias

**Lo que se vuelve más fácil.** El eje que hoy tiene un valor —`ACCEPT_ALL_DEFAULT`— pasa a tener el
que la forma diga, y con él arranca la comprobación que ya está escrita y desconectada (ADR-0055).
Desaparecen tres fealdades sin añadir gramática nueva salvo `use` y `base`: el prefijo interpolado, el
nombre decorativo y el IRI sin comprobar. Y el lenguaje gana algo que **nadie más tiene**: una errata
en un nombre de clase es un error de compilación.

**Lo que se paga.** Un programa sin documento y sin tipos escritos se queda sin nombres que resolver,
así que o declara sus tipos o escribe IRIs completos: **sin proveedor el lenguaje es más verboso, no
menos.** Esa es la barrera de entrada del primer `.fossil` de alguien, y es deliberada. Segundo: el
tipo tendrá que llevar bits que no son «forma» para poder emitir —node kinds, targets, `EXTRA`,
severidad— y si no tienen sitio, la emisión queda subespecificada; es la lección de DCTAP, que
necesitó hojas de extensión para poder escribir SHACL. Tercero: los comentarios, el orden y el
formato del `.shex` importado **no van a sobrevivir** al viaje, porque rudof nos da un AST y no un
CST con trivia. Hay que decirlo antes de que alguien lo reporte como fallo — Prisma publicó su lista
de preservación en términos del fichero cuando la garantía era del AST y se comió años de bugs.

**Lo que la reabre.** Que el porcentaje de shexTest se atasque por debajo de lo aceptable: eso diría
que el tipo del lenguaje no puede representar ShEx y que el documento tiene que volver a ser la
autoridad. Y en el otro sentido, un cliente que llegue con SHACL escrito (ADR-0055 §«lo que la
reabre»), porque la intersección expresiva de las dos lenguas es estrictamente menor que cualquiera
de ellas y el modelo canónico no puede ser el punto de encuentro.

**Lo que no se investigó.** El cuarto frente del estudio —type providers de F# y la frontera entre
tipo y atributo— murió por límite de sesión. Lo que decidía era **si `use` se declara en el programa
o fuera de él**; esta ADR elige dentro, con el apoyo indirecto de Smithy, TypeSpec, Protobuf y CUE,
que declaran sus imports en el fichero, y con el resultado negativo de LinkML y GraphQL, que no los
declaran. Si ese frente se retoma y dice otra cosa, el §1 es lo que hay que releer.


---

## Enmienda, 2026-08-08 — cuatro hechos del propio árbol y una reformulación

Cuatro frentes de investigación más volvieron después de escribir lo de arriba. Tres cosas que
traen son del repositorio, no de la web, y están verificadas contra el fichero:

**1. `use` ya existe en la gramática, así que §1 cuesta menos de lo que esta ADR decía.**
`grammar.bnf:104-111` ya define `Import := 'use' Path SelectiveImport? Alias?` con
`PathSegment := IDENT | STRING`, o sea que `use <personas.shex> as personas` casi parsea hoy. Sólo
aparece en un fixture del parser (`use stdlib/seq { filter, map }`) y en ningún programa real. El §1
no inventa una forma: enciende una que estaba escrita y apagada.

**2. El sigilo de atributo ya está elegido, y no es `#[...]`.** `grammar.bnf:85-87` tiene
`AT_ATTR := '@' IDENT` con el comentario *«Future attributes (@dcat, @prov, @chunked, etc.)»*. Y `#`
es hostil aquí por tres razones acumuladas: un `#` suelto es **inlexable y colgaría el parser**
(cabecera de `tests/fixtures/canonical_200.fossil`), es el carácter de comentario de Turtle y SPARQL
—los dos idiomas que un lector de fossil lleva en la cabeza—, y es el separador de fragmento de medio
vocabulario RDF. Pero **el sigilo NO queda decidido aquí**, y `@` tampoco está libre: ya carga dos significados —
`@export` como modificador de definición (`grammar.bnf:86`) y, sobre todo, **`@conn` como alias de
conexión**, que es la pieza de keasy y aparece en todos los ejemplos publicados
(`io.csv("@examples/ecommerce-customers.csv")`). Formalmente no hay conflicto de parseo, porque el
alias vive **dentro de un literal de cadena** y un atributo iría a nivel de ítem; pero para el lector
`@` significaría tres cosas en un mismo fichero. Eso es la regla 2 de la casa —una grafía, una idea—
así que el sigilo queda **abierto**, con tres salidas y ninguna elegida: `@nombre(...)` aceptando la
homonimia, otro sigilo que haya que buscar (y `#` está descartado por lo de arriba), o **ninguno**:
atributos como declaraciones ordinarias con palabra clave, que es lo que menos gramática pide y lo
que mejor casa con la estética que este rediseño persigue. Lo decide Angel, no esta ADR.

**3. «Anotación» ya significa otra cosa dentro de fossil.** `type-system.md:250` y
`grammar.bnf:144-153` la usan para la reificación de RDF 1.2 (*statements about statements*). El
vocabulario de esta ADR dice «atributo», nunca «anotación».

**4. Y una contradicción de frente que hay que resolver, no esconder.**
`apps/docs/content/docs/characteristics/types.mdx:121` lleva un callout *«Deliberately absent»* que
dice, literal: **«No user type annotations — types come from descriptors.»** El §2 de esta ADR —que
el tipo se pueda escribir en el lenguaje para que el documento sea opcional— **lo deroga**. La
decisión se mantiene, porque el requisito («el documento no tiene por qué ser obligatorio, sino
representable en el lenguaje») es explícito de Angel y es lo que hace que el `.shex` sea un origen y
no una autoridad. Pero **la página tiene que cambiar con la implementación**, y hasta entonces el
repositorio afirma dos cosas incompatibles en dos sitios.

**La reformulación del §5.** El criterio original —«los atributos nunca llevan identidad»— no
sobrevive al contraejemplo: Cap'n Proto dice *«any symbolic name can be changed, as long as the type
ID / ordinal numbers stay the same»*, y bajo esa lente un `@iri(foaf:name)` **es** la separación
nombre≠identidad que esta ADR defiende citando protobuf, con el identificador local como etiqueta
renombrable. El criterio correcto es el que ya usaba el §6: **¿es un nodo del modelo o es trivia?**
La identidad tiene que ser nodo. Lo que sigue prohibido, y ahora con dos medidas encima, es
**derivarla de un nombre**:

- Ninguna regla de casing puede acercarse a un IRI. El menú de `rename_all` de serde tiene 8 de las
  12 combinaciones posibles y no puede expresar `Content-Type`, `ETag`, `$ref` ni `_meta`; con
  identificadores no ASCII —`edad`, el ejemplo de esta ADR— **panica el compilador** (serde#2953) o
  es un no-op silencioso. Y RDF sólo ha adoptado **concatenación**, jamás transformación: `@vocab` de
  JSON-LD y owlready2 anteponen la base al término tal cual.
- **Pinto**, mapeador RDF de convención pura, emitía `<tag:complexible:pinto:name>`: triple
  sintácticamente perfecto, valor de interoperabilidad **cero**. Muerto desde 2019, y sus propios
  docs conceden el arreglo por atributo.
- La convención no es propiedad de tu esquema sino **de quién posee los nombres** — medido sobre 951
  crates: 0 overrides en 798 campos cuando el vocabulario es tuyo, 20,6% cuando consumes el de otros.
  En RDF nunca posees los nombres.

**Y un aviso operativo que corrige lo que esta ADR insinuaba sobre resolver vocabularios:** prefix.cc
sirve un certificado caducado desde el 2025-12-31 —siete meses— mientras todos los IRIs que abrevia
responden. Los IRIs son duraderos; la capa de abreviatura se pudre. **Resolver un prefijo por red en
tiempo de compilación queda prohibido**; como ayuda del editor, es otra cosa.

**Deuda encontrada de paso, que no bloquea nada:** `PrefixedName := IDENT SHAPE_SEP LocalName` con
`IDENT := (LETTER|'_')(LETTER|DIGIT|'_')*` significa que fossil **no puede escribir hoy**
`dc:title-alt`, `ex:1234` ni ningún local con `.`, `-`, `%` o escapes — un subconjunto estrecho de
`PN_LOCAL` de Turtle. Cualquier `.shex` de un tercero con esos nombres es hoy inexpresable.

**Lo que sigue sin investigarse**, y por segunda vez: el frente de F# type providers y el de esquemas
de base de datos (sqlx, Diesel, Prisma, jOOQ) se perdieron enteros con la sesión. Lo que decidían era
el coste operativo de declarar el proveedor dentro del programa — red o disco durante la comprobación,
hermeticidad de la build, comportamiento en el LSP. El §1 sigue en pie sobre el apoyo indirecto que
ya tenía, más el hecho nuevo de que la forma ya está en la gramática.
