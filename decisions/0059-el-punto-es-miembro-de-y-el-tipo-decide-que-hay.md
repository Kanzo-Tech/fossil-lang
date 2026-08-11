# ADR 0059: El punto es «miembro de», y el tipo decide qué hay

**Date:** 2026-08-11
**Status:** proposed
**Decider:** Ángel Iglesias (Kanzo)
**Cite:** ADR-0058 (la identidad); ADR-0057 (7ª: el proveedor es un constructor y la interpolación; 9ª: la referencia cualificada y `as`; 10ª: el binding es posicional); ADR-0055 (el checker lee el documento de forma); F# type providers — PLDI 2016 «Types from data» §6.3 y §6.5; Rust `str::len(&s)` ≡ `s.len()`; Python `str.upper(s)` ≡ `s.upper()`; Prisma `@map`; plan F2 («lo que el HIR no necesite sale de la gramática») y su regla de catálogo.

## Context

ADR-0058 cerró la identidad. Queda la superficie entera, y todas sus piezas
resultaron ser la misma pregunta: **qué significa el punto.**

Hoy el lenguaje tiene tres cosas que se parecen y no son iguales — `ex:name` para
una clave de propiedad, `clean.slug` para una función de la stdlib, `users.name`
para una columna — y dos operadores para ligar, `=` para tipos y `:=` para
valores, sin que la distinción corresponda a nada. Además `where`, `select` y
`join` son verbos con gramática propia mientras `seq/` tiene trece funciones
equivalentes en el catálogo: dos mecanismos para una idea.

Y el lenguaje **es estáticamente tipado**. Tiene retículo, comprobación
bidireccional y un checker que lee la forma de salida. El único sitio donde no se
usa el tipo para lo que un tipo sirve —decidir qué se puede hacer con algo— es
justo el despacho de la stdlib, que va por cadena punteada.

## Decision

### 1. `.` significa «miembro de», y es el único operador de acceso

Lo que hay a la izquierda decide qué se obtiene:

| a la izquierda | miembros |
|---|---|
| un **espacio de nombres** (`io`, `clean`, `parse`, `validate`, `math`, `anon`) | sus funciones |
| un **tipo** (`User`, `Person`) | sus campos |
| un **iterable / relación** (`Users`, `Adults`) | sus operaciones (`where`, `select`, `join`) |

No hay `::`. La distinción de Rust entre camino en compilación y acceso en
ejecución **no tiene referente aquí**: un programa fossil no ejecuta nada,
`User.age` es una referencia a columna que acaba siendo SQL. Importar dos
operadores sería importar una diferencia que no existe.

### 2. `seq` y `str` son tipos, no espacios, y sus miembros se alcanzan por dos caminos

`seq.filter` es una operación sobre una secuencia y `str.concat` sobre una
cadena: son **miembros**, no funciones de un cajón. Y se llega a ellos por el
valor **o** por el tipo:

```
email = User.email.trim().lower()
email = str.lower(str.trim(User.email))
```

Es **una entrada del catálogo por dos caminos**, no dos formas de expresar dos
ideas — que es exactamente `s.len()` ≡ `str::len(&s)` en Rust y `s.upper()` ≡
`str.upper(s)` en Python. La regla 2 de la casa prohíbe dos maneras de decir lo
mismo; esto es una regla de resolución, y el precedente es de los dos lenguajes
que más gente lee.

**El coste aceptado:** el corpus acabará conteniendo las dos, y habrá que
explicar una vez que son la misma.

### 3. Los verbos son entradas de catálogo, no gramática

`where`, `select` y `join` dejan de tener producciones. Un verbo nuevo es **una
fila**, no una regla — que es la regla que el plan F ya lleva escrita: *toda
capacidad nueva entra como entrada de catálogo, nunca como producción*. Y
`RegistryEntry` gana un tipo de receptor: el despacho deja de ser por cadena y
pasa a ser por tipo.

De paso muere la desambiguación que costó el commit `f9ee991`: `clean.slug` sin
paréntesis era «una función nombrada y no aplicada» sólo porque `clean` era un
espacio que no era un valor. Con miembros tipados, esa rama es innecesaria.

### 4. `io` es el espacio de los proveedores

`io.csv`, `io.shex`, `io.rdf`, `io.parquet`. **No tienen receptor porque crean la
cosa.** Añadir GraphQL o LinkML mañana es `io.graphql` — una fila, sin tocar el
núcleo. Es el registro de proveedores en el sentido de F#.

### 5. `:=` liga, `=` asigna

Un solo binder. `type { Person } := io.shex(…)` y `User := io.csv(…)`: lo que se
liga lo dice la izquierda. `=` queda para lo que de verdad es una asignación —
las propiedades del cuerpo y `@subject`.

### 6. Un binding liga el tipo y la relación

`User := io.csv("data/users.csv")` liga **las dos cosas con un nombre**:
`User.age` es su campo, `from User` son sus filas. F# separa
`CsvProvider<…>` de `.Load(…)` porque allí un tipo se reutiliza con muchos
ficheros; aquí un binding **es** una fuente concreta, así que separarlos sería
ceremonia sin trabajo.

Consecuencia de nombres: `.` sobre un tipo da campos; `Adults` es una relación de
`User`s, y por eso dentro del cuerpo se escribe `User.name`, no `Adults.name`.

### 7. El mapeo conserva su nombre

`Users : Person from Adults`. Dos nombres —el del mapeo y el del tipo— porque dos
fuentes pueden producir `Person` y un diagnóstico tiene que poder decir cuál.
Como el nombre se queda, **el `:` final que abriría el bloque sobra**: lo abre la
indentación, como hoy, y dos `:` en una cabecera serían ruido.

### 8. Las claves de propiedad son nombres desnudos

`name = User.name`, no `ex:name = …`. El nombre corto sale del **último segmento
del IRI del predicado** que declara la forma — que es lo que `fossil-mir` ya hace
hoy por dentro (`local_name(iri)`), sólo que pasa a ser lo que se escribe.

**Reglas de derivación, y salen de una medición ajena:** el nombre depende **sólo
de la etiqueta** — nunca de la cardinalidad, del tipo ni de la posición — y **no
hay heurística de plural/singular**. El artículo de PLDI 2016 de los autores de
FSharp.Data declara en §6.5 que su algoritmo evita todo lo que haga que *«un
cambio pequeño en la muestra cause un cambio grande en los tipos provistos»*, y
**no aplicó ese principio a los nombres**: su esquema de colisión es un sufijo
numérico (`PascalCase2`), su singularizador produjo `Purchasis` y `Sourcis`, y un
cambio de versión menor renombró miembros y rompió una aplicación en producción.
No hace falta argumentar contra el sufijo numérico desde primeros principios:
basta su propio criterio.

**Colisión ⇒ error**, nombrando los dos IRIs. Nunca desambiguar sola. La
reparación es un atributo sobre el binding del tipo, en la forma de `@map` de
Prisma — todo constantes, encima de la declaración:

```
@rename(Person, "http://xmlns.com/foaf/0.1/name" as foaf_name)
type { Person } := io.shex("shop.shex")
```

Va en el programa y no en el `.shex` porque el vocabulario puede no ser tuyo.

### 9. Una arista se declara nombrando el tipo destino

`buyer = Person(User.email)` — «la Person cuya identidad se construye con este
email». El lowering conoce el tipo y usa **la** plantilla de ese tipo, que
ADR-0058 hace única. Con eso muere `subject_skeletons`: hoy una arista se deduce
comparando *esqueletos* de plantilla entre mapeos, con un marcador `\u{1}` de por
medio, y por eso `clean.slug(.name)` contaba como constante y podía casar donde
no debía.

## El programa entero

```
type { Person, Order } := io.shex("shop.shex")

User     := io.csv("data/users.csv")
Purchase := io.csv("data/orders.csv")

Adults := User.where(User.age >= 18)

Users : Person from Adults
    @subject = "https://shop.example/user/{User.email}"
    email    = User.email
    name     = User.name

Orders : Order from Purchase.join(User, on = Purchase.user_id == User.id)
    @subject = "https://shop.example/order/{Purchase.id}"
    total    = Purchase.amount
    buyer    = Person(User.email)
```

## Consequences

**El completado deja de ser aproximado.** Hoy escribes `clean.` y salen las
funciones de limpieza aunque el valor sea un entero. Con miembros por tipo, el
punto después de un `String` ofrece **exactamente** lo aplicable, y ofrecer algo
que no compila deja de ser posible. El `did_you_mean` de miembro desconocido
busca entre los miembros de ese tipo en vez de entre las 50 entradas del
catálogo: sugerencia mejor, por el mismo cambio.

**Y hay un code action que este diseño hace posible y que hoy no lo es.** En
cuanto se escribe `Users : Person from Adults`, el servidor sabe qué propiedades
declara `Person` y con qué cardinalidad — la tabla de restricciones ya está en el
checker desde ADR-0055 — así que puede rellenar el cuerpo entero con un hueco por
propiedad:

```
Users : Person from Adults
    @subject = ▮
    email    = ▮
    name     = ▮
```

Los nombres los pone la forma; los huecos los rellena el autor. Y dentro de cada
hueco, el completado ofrece los campos de `Adults` — que es un `User`, y `User`
es un tipo con nombre porque lo liga su binding. **Las dos mitades del mapeo se
completan desde los dos extremos y ninguna necesita heurística: es el tipo
respondiendo.** Es la contrapartida en el editor de «esto es un lenguaje
estáticamente tipado», y es `codeAction`, que `fossil-lsp` ya sirve. El
precedente es «implement missing members» de rust-analyzer.

**Lo que muere:** `prefix` y los CURIEs; `PropertyKey::PrefixedName`; los verbos
como gramática; `subject_skeletons` y su marcador; `as` como forma normal de
escribir (queda sólo para el self-join, que es lo que la 9ª ya decía); y la fila
anónima con la síntesis implícita de clausuras que la sostenía —
`expr_contains_free_field_refs`, `rewrite_field_refs_to_row_dot` y
`synthesize_closure` existen **sólo** porque el predicado no nombraba su fila.

**Lo que se paga:** el corpus tendrá los dos caminos de la decisión 2; `prefix`
sólo puede quitarse después del corte del descriptor de salida, o se cambia un
compromiso con RDF por otro escrito distinto; y los 68 ficheros `.fossil`
versionados se reescriben **una vez, al final** — no 35, que es lo que decía la
9ª enmienda contando un árbol que ya no es éste.

**Lo que revertiría esto:**

1. Un tipo cuyo conjunto de miembros no se pueda decidir estáticamente. Todo el
   diseño descansa en que el receptor se conoce en compilación.
2. Que los dos caminos de la decisión 2 se usen para cosas distintas en el
   corpus. Si aparece un idiom donde `str.lower(x)` significa algo que
   `x.lower()` no, dejan de ser una regla de resolución y pasan a ser dos
   spellings, que es lo que la regla 2 prohíbe.
3. Un vocabulario real cuyos nombres cortos colisionen tanto que `@rename` sea la
   norma en vez de la excepción. Ahí el nombre corto deja de ser el nombre.

**Lo que queda abierto:** dónde vive `select` cuando la relación viene de un
join; y si `io` debería nombrar también el destino, que hoy no tiene sintaxis
ninguna.
