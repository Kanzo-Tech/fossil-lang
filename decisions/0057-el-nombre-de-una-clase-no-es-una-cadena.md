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

**1. `use` ya existe en la gramática — pero no sirve tal cual, y esto lo corrige.**
`grammar.bnf:104-111` define `Import := 'use' Path SelectiveImport? Alias?`, y sólo aparece en un
fixture del parser (`use stdlib/seq { filter, map }`), en ningún programa real. *(Corrección del
2026-08-08: la redacción original decía que `use <personas.shex> as personas` «casi parsea hoy». **No
parsea**: `PathSegment := IDENT | STRING` y `<personas.shex>` lexa como `ABS_IRI`, que
`parse_path_segment` manda a recuperación.)* Y hay tres desajustes más, medidos, que el §1 tiene que
resolver al encenderla:

- **`SelectiveImport := LBRACE IDENT (COMMA IDENT)* RBRACE` no puede deletrear `ex:Person`.** Para
  importar miembros comprobados de un documento de formas necesita `IRIExpr`, no `IDENT`.
- **El ancla de la segunda enmienda no tiene sitio.** `Alias?` no la cubre: hay que añadir producción.
- **`Path` con `/` es una noción de módulo** que choca con el §4 de la segunda enmienda, donde la
  ruta se resuelve fuera. O se quita el `/`, o se quita esa parte del §4.

Así que el §1 no enciende una forma escrita: **la reescribe**, con la mitad de la producción actual
tirada. Sigue siendo más barato que inventarla, y sigue sin ser gratis.

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
así que el sigilo quedó abierto un tiempo. **Está decidido: `@nombre(...)`. Ver la cuarta enmienda.**

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
  12 combinaciones posibles y no puede expresar `Content-Type`, `ETag`, `$ref` ni `_meta` — los
  cuatro aparecen literalmente en un corpus de 951 crates. Y con identificadores **no ASCII** panica
  el compilador (serde#2953). *(Corrección: la redacción original ponía `edad` de ejemplo y `edad` es
  ASCII y no panica; el caso es `año`.)* Y RDF sólo ha adoptado **concatenación**, jamás
  transformación: `@vocab` de JSON-LD y owlready2 anteponen la base al término tal cual.
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

---

## Segunda enmienda, 2026-08-08 — el `use` va anclado

Un frente adversarial fue a refutar el §1 («el proveedor se declara dentro del programa») y volvió con
un **no**: lo que refuta es una versión más débil, **«dentro, resuelto en vivo, sin ancla»**, que es
exactamente lo que el §1 decía. Dos datos cortan el ataque de raíz:

- La sección *«Mistakes and Questions»* de *The Early History of F#* (Syme, HOPL IV 2020) enumera el
  código cerrado, SRTP, la comparación genérica y la precedencia de `<|`. **Los type providers no
  aparecen.** El retro «buena idea, mal sitio» que buscábamos no existe.
- Todo el daño documentado de F# es de **liveness**, no de ubicación: la deriva de esquema es
  invisible y se arregla con un *Clean* a mano; la señal de invalidación **se ignora en `fsc.exe`** y
  sólo la respeta el IDE, o sea que el compilador de CI nunca revalida; la red caída es un fallo de
  compilación; el esquema se relee en cada recompilación. Y F# publicó el ancla **dentro del
  programa**: `LocalSchemaFile` con `ForceUpdate=false`.

Y «fuera» no sale mejor: `sqlx` exige una base de datos viva en tiempo de build, y su arreglo **no es
la ubicación sino el anclaje** — `cargo sqlx prepare` escribe `.sqlx/`, que se commitea. Diesel igual,
con `schema.rs` commiteado y el precio de dos fuentes de verdad. C# estudió los providers y entregó
*source generators*, deliberadamente lo contrario: sólo aditivos, y **su entrada es la compilación, no
el mundo**. Nadie está puro en ningún extremo: todos parten la cosa en **nombre dentro, ubicación
fuera, identidad anclada** — Cap'n Proto (`using … import` con `-I`), Go (`import` + `go.sum`), Nix
(`inputs` + `narHash` en el lock).

**Enmienda al §1, en cinco puntos:**

1. **`use` se queda dentro.** No refutado.
2. **`use` lleva ancla**, y el hash es de la **forma normal RDFC-1.0**, no de los bytes. Es la lección
   de Dhall —*«los chequeos son semánticos… no rechazan cambios que preservan el comportamiento, así
   que refactorizar o tocar espacios y comentarios no altera el hash»*— y encaja con lo que el §7 ya
   decidió: si el formato del `.shex` no sobrevive al viaje, un hash textual churnea en cada
   reformateo y uno semántico no.
3. **Caché direccionable por contenido**, y un import anclado se resuelve de caché **sin red**. La
   regla de Dhall es la correcta y es simétrica: *un import sin ancla no se cachea*.
4. **La ruta se resuelve fuera** (un *search path* estilo `-I`). Dentro va el nombre y la identidad;
   fuera, dónde están los bytes — así, mover o vendorizar el documento no edita el programa.
5. **Dependencia por contenido y fallo ruidoso.** `addDependentFile` de Template Haskell recompila por
   *contenido del fichero, no por mtime*; F# hizo esto mal y por eso `fsc` no ve la deriva. Un `use`
   que no resuelve es un error, nunca una degradación.

**Y un detalle barato que arrastra al §6:** F# documenta que *«los type providers no pueden emitir
avisos»* — todo diagnóstico suyo es una excepción. Nosotros necesitamos **canal de aviso**, que es por
donde tienen que salir los `Unsupported(...)` del §6 sin abortar la compilación.

**Un precedente que es literalmente nuestro caso**, y elige *fuera al generar, dentro al usar*:
`DefinedNamespace` de rdflib lleva en la docstring *«Generated from: http://xmlns.com/foaf/spec/…
Date: 2020-05-26»* — el vocabulario se leyó **una vez**, en tiempo de autoría, y el artefacto se
commiteó con procedencia. Con dos matices que nos tocan: su defecto es **avisar, no fallar**
(`_warn = True, _fail = False`; sólo FOAF y SH piden fallar), y tiene `_extras`, una lista manual de
nombres que el lenguaje anfitrión no sabe deletrear — que es nuestro §6 un nivel más abajo, y también
la deuda de `PN_LOCAL` de la primera enmienda.

**Lo que reabre esto:** si el proveedor deja de ser un fichero y pasa a ser un servicio vivo (un
endpoint SPARQL, un registro de formas), el ancla deja de ser opcional y el perfil de coste pasa a ser
el de `sqlx`.

**Sin verificar:** por qué Idris 2 retiró los type providers que Idris 1 tenía tras
`%language TypeProviders` (el hecho está confirmado; la razón, no), la postura de Scala, y cualquier
medición del coste de recompilación de F# más allá de la afirmación en `csharplang#236`.


---

## Tercera enmienda, 2026-08-08 — el criterio del §5, otra vez, y esta sí es comprobable

El segundo frente adversarial fue a por «la identidad nunca va en un atributo» y confirma la
reformulación de la primera enmienda — pero la aprieta con una objeción justa: **«¿nodo del modelo o
trivia?» es correcto y no es comprobable.** Nadie puede mirar una línea y decidir. Los dos criterios
que sí lo son:

**Regla A — Lo prohibido no es el atributo: es el *fallback* derivado.** Y esto obliga a corregir el
contraejemplo de la primera enmienda: **Cap'n Proto deriva del nombre igual que LinkML.** Su
referencia dice las dos cosas — *«any symbolic name can be changed, as long as the type ID stays the
same»* y también *«**you cannot change the name of a type that doesn't have an explicit ID, as the
implicit ID is generated based in part on the type name**»*. O sea: es el modelo LinkML con un escape
explícito, y su único mérito es documentar el precio, que LinkML no documenta.

**Regla B — Un atributo obligatorio en toda declaración no es un atributo: es sintaxis con
corchetes.** RDFBeans lo demuestra: `@RDFBean("foaf:Person")` y `@RDF("foaf:name")` son requeridas, y
sin ellas la clase no se reconoce. Un `@iri(...)` obligatorio **es** `ex:name =` con más ruido, y la
elección pasa a ser de longitud de línea. Uno **opcional** reintroduce el fallback y cae bajo la
Regla A. Las dos juntas resuelven el §5 entero: **si el atributo se puede omitir, está prohibido; si
no se puede omitir, es sintaxis y la grafía da igual.**

**Regla C — lo que no sobrevive a la exportación es el espacio de extensión ajeno al lenguaje, no el
atributo.** Smithy lo prueba en las dos direcciones: sus traits *de usuario* no llegan a OpenAPI,
pero el suyo propio `jsonName` —que es identidad pura— **sí se convierte**. Y formula la frontera
mejor que esta ADR: *«shapes provide the structure and layout of an API, while traits provide
refinement and style»*.

### Los dos argumentos prestados, corregidos

**Protobuf decía lo contrario de lo que esta ADR le hizo decir.** El número es identidad **sólo en el
formato binario**; en cuanto llegaron JSON y TextFormat el nombre pasó a ser identidad y tuvieron que
añadir **reserva de nombres** (*«you can also reserve the field name to allow JSON and TextFormat
encodings to continue to parse»*), y `buf` mantiene por eso una categoría de rotura aparte,
`WIRE_JSON`, con la regla `FIELD_SAME_NAME`. Y protobuf **sí** pone un nombre de cable en una opción:
`[json_name = "..."]`. Conclusión correcta: protobuf apoya **«la identidad es explícita y
permanente»**, y no dice nada sobre dónde se escribe. Para nosotros el remate es que **RDF está
direccionado por nombre: el nombre ES el número.**

**GraphQL estaba mal aplicado.** El issue es de **2017** —nueve años, no cinco— y el RFC concreto
(#1075) se cerró sin fusionar en 2024. Pero sus ejemplos motivadores son `@sensitive` y `@cache`, que
es exactamente lo *accesorio* que el §5 manda a atributos: GraphQL no distingue identidad de trivia,
**no sobrevive nada**. Es evidencia sobre espacios de extensión de usuario en una especificación con
implementaciones independientes, situación en la que fossil no está porque posee compilador y emisor.
Lo que sí vale es el remedio: la federación de Apollo sirve `_service { sdl }`, **el texto fuente en
crudo**, porque las directivas no viajan. Cuando la anotación no viaja, se acaba enviando el programa
entero. *(Y queda prohibido el argumento OWL — «annotations are treated as not being present» habla
de `owl:AnnotationProperty` en el grafo, no de atributos de un lenguaje. Sería el mismo equívoco de
palabra que la primera enmienda ya detectó, al revés.)*

### La evidencia de dominio que faltaba, y por qué no nos aplica

**Los tres mapeadores objeto-RDF de Java ponen el IRI en una anotación**: JOPA (vivo, último push
2026-08-06) con `@OWLClass(iri=…)`, RDFBeans con `@RDF(…)` obligatoria, y Pinto —el de la primera
enmienda— que además tiene una anotación llamada literalmente `@Iri`. Tres de tres a favor del
atributo… **por una razón léxica: Java no puede deletrear un IRI.** fossil sí: `ex:name` es un token
del lenguaje. El precedente no transfiere, y por el mismo motivo tampoco transfiere `serde(rename)` —
medido, **el 53,4% de sus usos son escapes del léxico anfitrión** (identificadores Rust ilegales o
palabras reservadas).

**Y JOPA contesta la duplicación esquema↔programa mejor que nadie de su familia:** un plugin genera un
fichero `Vocabulary` desde la ontología *«para que puedan usarse en las anotaciones de mapeo»*. O sea:
**generar símbolos y referenciar el símbolo, nunca repetir la cadena.** `use` hace eso en el lenguaje
y sin codegen, que es estrictamente mejor.

### Lo que se añade al §1, y es normativo

**`import schema` de XQuery/XSLT es el §1, estandarizado desde 2007.** Importas un esquema, escribes
un nombre cualificado corto, el compilador lo resuelve contra las declaraciones importadas, y un
nombre desconocido es **error estático `err:XPST0008`**. XSLT 3.0 §3.15: *«names of such components
used statically within the stylesheet must refer to an in-scope schema component»*. La identidad allí
es un QName expandido — isomorfo a un IRI. En RDF nadie lo hace: el `IMPORT` de ShEx tiene la
maquinaria de ámbito pero sólo para **etiquetas de forma**, no para predicados, y los prefijos no se
importan. La pieza está a medio construir en la lengua que ya leemos.

**Y una condición de diseño que hay que respetar desde la primera línea:** `use <personas.shex> as p`
**cualificado desde el día uno**. LinkML asumió nombres únicos entre imports y lleva desde 2024
atascado en `structured_imports`; retrofitar la cualificación es la clase de cosa que no se retrofita.

### El precio de comprobar, ya con número

Del frente que se había perdido dos veces: `sqlx` tiene **32,5M de descargas en 90 días frente a 6,1M
de Diesel** — comprobar le gana 5:1 a generar, en Rust. Su dolor está catalogado por su propio
mantenedor: compilación lenta, macros que no son puras *«porque su salida puede cambiar entre
ejecuciones según cambie el esquema, sin que el compilador ni el IDE se enteren»*, y olvidarse de
`cargo sqlx prepare` antes de commitear. Los type providers de F# **siguen vivos** (FSharp.Data con
push del 2026-08-05) y su dolor es el IDE: `dotnet/fsharp#19368`, *«sustained high CPU usage
(~150%)»*, y el PM de F# reconociendo que regenerar en cada pulsación es *«a band-aid around some more
fundamental architectural flaws»*. **Traducción para nosotros: nuestro proveedor es un fichero, no una
red, así que nos libramos de casi todo — lo que sí pagaremos es el IDE y la invalidación de caché.**

### El coste del fallback derivado, medido en el vocabulario que mejor conocemos

schema.org deriva el IRI del nombre y por eso **no puede renombrar**: 82 términos con
`schema:supersededBy`, de los cuales **24 son renombrados puramente morfológicos**
(`actors→actor`, `reviews→review`), hay cadenas de dos saltos, y hay **una errata convertida en
identificador global permanente**: `clincalPharmacology → clinicalPharmacology`. Ninguno se borró.
Eso es lo que cuesta, en el vocabulario más usado de la web.

**Y una ley de serde que vale como aviso a futuro:** cuando un atributo empieza a llevar semántica,
converge en ser una expresión de tipos — es lo que le pasó a `with`, que no componía, y el arreglo
(`serde_as`) acabó reflejando la estructura del tipo dentro de una cadena. Corolario: **si `@iri(...)`
alguna vez tiene que llevar algo más que una constante, ha dejado de ser un atributo.**


---

## Cuarta enmienda, 2026-08-08 — el sigilo es `@`, y el subtipado existe

Dos decisiones de Angel, y las escribo con la cadena entera porque las dos se tomaron sobre evidencia
acumulada en tres enmiendas y ninguna de las dos se entiende leyendo sólo su conclusión.

### 1. El sigilo de atributo es `@nombre(...)`

**Lo que la decisión ya no es.** La Regla B de la tercera enmienda la había encogido antes de tomarse:
si un atributo es **obligatorio** en toda declaración, no es un atributo — es sintaxis con corchetes,
y `@class(ex:Person)` es `ex:Person` con más ruido. La elección de sigilo sólo pesa en los atributos
**opcionales**, que por la Regla A son los que llevan accesorios (documentación, sensibilidad de
F4 §3) y nunca identidad. Así que esto es una decisión de estilo sobre una superficie pequeña, no la
decisión de fondo que parecía dos enmiendas atrás.

**Por qué no `#[...]`, que era mi propuesta inicial.** Tres razones acumuladas, todas verificadas
contra el árbol:

1. **Un `#` suelto es inlexable y colgaría el parser** — la cabecera de `tests/fixtures/canonical_200.fossil` lo
   dice literalmente, y viene de `06-07 deferred-items.md`.
2. **`#` es el carácter de comentario de Turtle y de SPARQL**, los dos idiomas que un lector de fossil
   lleva en la cabeza.
3. **`#` es el separador de fragmento de medio vocabulario RDF** (`rdf-syntax-ns#type`).

**Por qué `@` a pesar de la homonimia, que fue una objeción de Angel y era correcta.** `@` ya lo usan
las **conexiones tipadas de keasy** — `io.csv("@examples/ecommerce-customers.csv")`, en todos los
ejemplos publicados. Pero las dos capas **no se cruzan léxicamente**: el alias vive **dentro de un
literal de cadena** y el atributo a nivel de ítem, así que no hay conflicto de parseo, sólo de
lectura. Y el otro ocupante de `@` desapareció: `@export` se borró en `c974fdd` — no exportaba a
nadie, porque no hay sistema de módulos. Queda además un beneficio operativo: mantener `AT_ATTR`
lexado impide que `@` se descarte en silencio, que es exactamente la clase de fallo del `#`.

**Por qué no «ninguno», que era mi recomendación.** Una palabra clave por atributo (`class ex:Person`,
`sensitive email: String`) parece menos gramática y es más en cuanto los atributos se acumulan: cada
uno se convierte en una palabra reservada del lenguaje. `AT_ATTR := '@' IDENT` es **una** producción
para todos los que vengan, presentes y futuros — que es la regla de catálogo-no-producción aplicada a
sí misma.

**Lo que este sigilo NO autoriza**, y hay que releerlo junto: los atributos siguen sin poder llevar
identidad si son omitibles (Regla A, con schema.org como coste medido: 82 términos con
`supersededBy` y una errata congelada como identificador global permanente). Y siguen sin poder
llevar semántica: la ley de serde de la tercera enmienda dice que un atributo que crece converge en
una expresión de tipos — `serde_as` es la prueba. **Si `@iri(...)` alguna vez necesita algo más que
una constante, ha dejado de ser un atributo.**

**Lo que la reabre.** Que aparezca un segundo ocupante de `@` a nivel de ítem, o que el alias de
conexión salga del literal de cadena — ahí sí habría colisión real, no homonimia.

### 2. Hay subtipado, y la página mentía

`apps/docs/content/docs/characteristics/types.mdx` afirmaba en un callout *«Deliberately absent … No
subtyping»* mientras `type-system.md` §9 define cinco reglas —`S-Refl`, `S-Opt`, `S-OptCov`,
`S-SeqCov`, `S-IntFlt`— que `compatible` (`crates/fossil-hir/src/check.rs:200`) **ejecuta en cada
propiedad**. El callout describía una intención; el checker hace otra cosa desde hace meses.

**Decisión: las reglas se quedan y la página se corrige.** No es preferencia estética — es dónde
estaba haciendo trabajo real el subtipado: `S-Opt` es lo que permite que una expresión no opcional
satisfaga una propiedad opcional (sin ella, cada una exigiría un `Optional` explícito), y `S-IntFlt`
es lo que permite que una columna `Integer` alimente una propiedad declarada `xsd:float` sin una
conversión escrita a mano. Quitarlas no simplifica el lenguaje: traslada el trabajo a cada programa.

Esto **no** contradice el resto del callout: sigue sin haber inferencia global y sigue sin haber
anotaciones de tipo de usuario *hoy* — esto último lo deroga el §2 de esta ADR cuando se implemente,
y ya está anotado en la primera enmienda.

**Lo que la reabre.** Que el subtipado empiece a interactuar con el álgebra de filas del §4 de
ADR-0054 de forma que dos columnas «compatibles» produzcan un join cuya clave no sea la misma en los
dos lados. Hoy el join exige `Primitive` **idéntica** a propósito; si eso se relaja a `<:`, hay que
volver aquí.

---

## Quinta enmienda, 2026-08-08 — un solo mecanismo: los tipos los da un proveedor

Deroga el §2 y reordena el §3. Es la conclusión a la que llegó Angel tirando del hilo del §1, y es
más simple que todo lo que esta ADR llevaba escrito.

**El §2 decía «el tipo se puede escribir en el lenguaje».** Eso metía un tercer mecanismo donde ya
había uno, y obligaba a derogar el callout de `types.mdx` que dice *«No user type annotations — types
come from descriptors»*. Se sustituye por:

> **Los tipos vienen siempre de un proveedor. Lo que varía es cuál.** El de la entrada son los datos
> (inferencia, ADR-0037); el de la salida, un documento de formas traído por `use`.

Consecuencias, y todas son borrados:

- **Muere `type X { … }`.** No hay tipos escritos a mano. Si algún día hay que tipar una entrada sin
  datos delante —un cliente que sube su CSV *después*, que es el caso real de keasy— eso **no** es
  escribir el tipo: es apuntar `use` a un descriptor de entrada. Un mecanismo, dos lados.
- **Muere la anotación de tipo en el binding** (`users : User := …`). Por lo mismo: nombraría algo que
  el proveedor ya nombra.
- **Y el callout de `types.mdx` deja de estar en contradicción con esta ADR** — vuelve a ser cierto,
  y la contradicción que la primera enmienda registró se cierra sin tocar la página.

**Sin proveedor de salida no hay contrato, y eso es una decisión, no un hueco.** Un programa sin `use`
escribe corpus igual: no se comprueba nada, y fossil **emite** la forma que dedujo — que es
exactamente lo que el §7 ya nos obliga a saber hacer. Lo que se pierde es la comprobación, que es lo
que el proveedor compra.

### Los prefijos, reordenados

El §3 pedía `base` como ítem de nivel superior junto a `prefix`. Con `use` trayendo al ámbito los
prefijos que el documento **ya declara**, no queda ningún prefijo de vocabulario que declarar. Sólo
sobrevive el de **acuñar sujetos** — y ése no es del fichero, es del mapeo, porque cada mapeo acuña en
su sitio (`/user/`, `/order/`). Así que no es `prefix` ni `base` a nivel de fichero: es un atributo
del mapeo, con el sigilo que la cuarta enmienda fijó.

```
use <personas.shex>

users := io.csv("data/*.csv")

@base(<https://example.org/>)
Person <- users
    iri  = /user/{.id}
    name = .name
```

**No aparece `prefix`. No aparece `type`. No hay un solo IRI escrito a mano fuera del `@base`.**

### Y la flecha, con una corrección

La refutación que la investigación trajo contra `Person <- User from users` iba dirigida a la forma de
**tres piezas**, donde `User` competía con la convención `nombre : Tipo` y rompía el despacho
name-first del parser. **Con el tipo de entrada fuera, ese argumento se queda sin objeto.**
`Person <- users` son dos cosas y una flecha; frente a `Person from users` es cuestión de gusto. De
aquella lista de seis razones sólo sobrevivía «`<-` es una segunda flecha teniendo `:=`», y `from` es
una palabra, no una flecha, así que ni esa distingue.

### Lo que queda abierto

- **Si `@base` acaba repetido en todos los mapeos**, en cuyo caso quiere subir a fichero y volvemos a
  tener dos sitios.
- **Qué emite exactamente un programa sin proveedor de salida** — la forma deducida es la dirección
  que ADR-0057 §1 rechazó *para comprobar*; emitirla es legítimo, pero hay que decir que ese documento
  no es un contrato, es un informe.

---

## Sexta enmienda, 2026-08-08 — la fila se nombra, y `<-` queda descartado

Investigación de sintaxis sobre cinco preguntas abiertas; cuatro volvieron verificadas.

**1. `<-` está descalificado por evidencia, no por gusto.** Haskell 2010 §3.11 define
`qual → pat <- exp` con `p :: t` y `e :: [t]`: **la izquierda es un ELEMENTO y la derecha la
COLECCIÓN**. Scala 2.13 §6.19 igual. Así que `Person <- users` dice, en el idioma del que tomaríamos
prestado el símbolo, que un `Person` es un elemento de `users` — al revés de lo que queremos. Y el
glifo ya significa asignación en R y recepción de canal en Go. **No se encontró un solo lenguaje
donde `<-` signifique «producido una vez por cada elemento de».** Además fluye derecha→izquierda
mientras `|>` fluye al revés, en el mismo fichero.

**2. La fila se nombra, y esto es el hallazgo del día.** Todos los sistemas que empezaron con receptor
anónimo tuvieron que añadir un mecanismo de nombrado **en cuanto hubo dos filas en ámbito**, y ninguno
hizo el camino inverso:

| sistema | empezó con | tuvo que añadir | disparador |
|---|---|---|---|
| XSLT/XPath | `.` (nodo de contexto) | `current()` | `.` queda tapado en predicados |
| XPath | sólo `.` | `for $x in` (2.0), `let $x :=` (3.0) | más de un ítem ligado |
| jq | `.` | `EXP as $x` | un valor de nivel superior |
| **R2RML** | `{column}` anónimo | `rr:child` / `rr:parent` | **join entre dos tablas** |
| Kotlin DSL | receptor implícito | `@DslMarker` | receptores anidados resolviendo mal |

Tres razones, en orden de peso:

- **Michael Kay, comparando XSLT y XQuery:** el ítem de contexto *«optimiza para navegación jerárquica
  en vez de para joins»*. fossil mapea fuentes **tabulares**: no hay jerarquía que descender, así que
  pagamos el coste del compromiso sin cobrar el beneficio.
- **El fallo es silencioso y lo dispara el DATO, no el código.** `./@ref` de XPath degrada calladamente
  a `@ref`; el `with` de Delphi hace que un registro que **gana un campo** cambie el significado del
  código que lo rodea — y Embarcadero documenta que a `TRect` le pasó de verdad con `Width`. En un
  lenguaje cuyo trabajo es sobrevivir a la deriva de esquema, eso es el peor modo de fallo posible.
- **Nuestro propio dominio ya perdió tres veces**: los `rr:child`/`rr:parent` de R2RML son un binder
  sin nombre —fijos, dos, y roles en vez de nombres—, YARRRML distingue los dos lados con etiquetas
  **posicionales** `s`/`o`, y ShExML cualifica ambos lados enteros.

**El coste es un nombre y un carácter por referencia.** Y desbloquea el join con claves de nombres
distintos que ADR-0054 dejó declarado y sin construir, sin necesitar «referencia cualificada» como
forma nueva.

**3. Dónde va el binder: al final, con `as`.** `personas.Person from users as u`. Es el
`SOURCE … AS ?var` de SPARQL-Generate y el `BIND(… AS ?x)` de Stardog, los dos hermanos más cercanos
del dominio. **Y no `from u in users`, aunque no costaría palabras nuevas** —`in` y `from` ya son
reservadas (`grammar.bnf:26`)— porque **`in` ya significa otra cosa en esa misma línea**:
`InClause := 'in' IRIExpr` es el grafo con nombre. Dos significados de `in` en una cabecera es la
regla 2 de la casa incumplida en el sitio más visible del lenguaje.

**4. `personas.Person` no colisiona con `.id`, y está probado formalmente.** La especificación de
`dot-shorthands` de Dart 3.10 §Non-ambiguity: el `.` infijo exige una expresión completa a la
izquierda, y el `.` inicial sólo aparece donde una expresión puede **empezar** — posiciones disjuntas,
*«no new grammatical ambiguities»*. Zig, Dart, Swift y Elm envían los dos a la vez. **Y la regla ya
está escrita en nuestra gramática** (`grammar.bnf:244-246`). Un solo borde real: una línea que empieza
por `.`, que Dart prohíbe de plano por recuperación de errores.

Con una salvedad: OCaml, Haskell y Elm se libran porque **exigen mayúscula inicial en los módulos** —
el lexer te dice de qué lado del punto estás. `personas` es minúscula y parece un campo. Regla
propuesta: **un nombre de `use … as N` queda reservado en todo el fichero** y colisiona con ruido si
una columna se llama igual. Alternativa si eso incomoda: `personas::Person`, la respuesta de Rust,
inequívoca contra los dos usos de `.`. Lo que **no** vale es `personas:Person`: está a un carácter de
`personas := …`.

**5. `from` se queda.** `for … in` importaría el orden fuente-primero que no necesitamos y reencuadra
una **regla** como un **bucle** — y XSLT, que lleva 25 años con las dos formas, concluyó que la de
regla es el mejor defecto y el bucle es la muleta. El `:-` de Datalog encaja mejor semánticamente y
está a dos píxeles de `:=`. Nota: el argumento de IntelliSense que se le atribuye a LINQ **es folclore
sin fuente publicada**, y en cualquier caso no nos afecta: nuestra proyección vive en el cuerpo, no
antes del `from`.

### Lo que sigue sin contestar

**Dónde va la declaración del sujeto — la pregunta 1 se perdió con un agente y no se reconstruye.**
Lo único recuperado: los atributos de bloque de Prisma (`@@id`, `@@map`) van **dentro, al final**, y
Django articula el criterio — *«model metadata is anything that's not a field»*. Falta el porqué
escrito por alguien, y faltan enteros Ecto, ActiveRecord, jOOQ, Diesel y Beam.

Y un aviso de método: los cuatro frentes que volvieron agotaron la cuota de búsqueda al arrancar y
trabajaron por fetch directo, lo que sesga hacia documentos normativos **y en contra de quejas de foro**
— que es donde vive el material de arrepentimiento.
