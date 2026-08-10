# ADR 0054: `join` entra en la primera versión, y es una igualdad por nombre

**Date:** 2026-08-07
**Status:** accepted — §§3-4 superseded by ADR-0057's ninth amendment (2026-08-10); §§1, 2 and 5 stand
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Cierra la pregunta que ADR-0050 §3 dejó abierta y que
`apps/docs/content/docs/characteristics/pipeline.mdx` §"What this does not settle" lleva como primer
punto. Abre F5 de ADR-0046. La forma la escribió `operator-algebra.md:186` — `s |> join(t, on=c)`.

## Contexto

F5 es la tubería de fuente: `adultos := users |> where(.edad >= 18)`, una **relación** y no un valor.
ADR-0050 desazucaró la tubería en posición de valor y se detuvo aquí a propósito, porque de los tres
verbos que F5 necesita —`where`, `select`, `join`— sólo el tercero obliga al checker a aprender algo
que hoy no sabe: **componer dos filas**. `where` preserva la fila de su única fuente y `select` la
restringe; ninguno de los dos es álgebra.

Lo que ya está en el árbol y lo que no:

- `Op::Join { left, right, on, kind, left_name, right_name }` existe desde la fase 4
  (`crates/fossil-mir/src/op.rs:84`) con `JoinKind` de cuatro variantes, y **ninguna es alcanzable
  desde el código fuente**: la bajada emite cuatro de catorce operadores.
- `NAMED_ARG := IDENT ASSIGN Expression` ya parsea en posición de llamada
  (`crates/fossil-syntax/src/parser/expr.rs:222`), así que `on = .k` no pide gramática nueva.
- El HIR tiene `FieldRef(SmolStr)` y nada más (`crates/fossil-hir/src/lower.rs:84`): **no existe la
  referencia cualificada**. `personas.id` no es una expresión del lenguaje.

Y el argumento de producto, que es el que decide: un mapping cuyos dos extremos de arista viven en
dos ficheros no se puede escribir hoy sin unirlos **fuera** de fossil, y ese fuera es exactamente lo
que la tubería existe para eliminar. La resolución de extremos que hace el backend (IRI → id denso)
es un join suyo, no del programa, y no sustituye a éste.

## Decisión

**1. `join` entra en la primera versión.** No se pospone a que `where` y `select` "basten un tiempo":
lo que basta un tiempo es una tubería que no compone fuentes, y una tubería que no compone fuentes es
la mitad de un pipe.

**2. Sólo `Inner`, y la razón no es pereza.** Un join externo **fabrica NULL**, y F4 —la forma es el
contrato— no tiene todavía manera de declarar una propiedad nulable: el descriptor de salida ni
siquiera llega al typecheck (`ACCEPT_ALL_DEFAULT`, `crates/fossil-hir/src/check.rs:96`). Un
`LeftOuter` antes que F4 mete nulos en un corpus cuyo contrato afirma que no los hay. Las otras tres
variantes de `JoinKind` siguen definidas y siguen inalcanzables; la que llegue, llegará con su
historia de nulabilidad y no antes.

**3. La condición es una igualdad y la clave se nombra una vez: `on = .k` ≡ `USING (k)`.** `k` tiene
que existir en las dos filas; en el resultado aparece una sola vez. No se admite una condición
arbitraria porque `.` significa *la fila* y no puede significar dos filas distintas dentro de la
misma expresión mientras no exista la referencia cualificada — que es una forma nueva del lenguaje,
no un argumento más.

**4. El álgebra de filas: `fila(izq) ⊎ fila(der)`, con `k` identificada, y cualquier otro nombre
compartido es un error** que nombra las dos fuentes. No hay shadowing y no hay cualificación
automática: una columna que aparece dos veces con dos significados es precisamente lo que ningún
mapping quiere descubrir en el corpus. De aquí sale gratis que **el self-join es un error** —
`a |> join(a, on = .k)` colisiona en todas las columnas menos la clave.

**5. Las dos `k` tienen que tener la misma `Primitive`.** Distinta primitiva es un error, no una
coerción, por la misma razón que las dos ramas del ternario en ADR-0050 §1: una coerción que nadie
escribió es un tipo que nadie comprobó.

## Consecuencias

**Lo que se vuelve más fácil.** `Op::Join` deja de ser un operador definido y nunca alcanzado, que es
la deuda que ADR-0009 aceptó explícitamente y esta ADR empieza a pagar. El checker gana su primera
regla de dos entradas, que es la que faltaba para que "el checker es lo nuestro y el plan es de
DataFusion" deje de ser una frase: DataFusion reordena joins, nosotros decimos qué fila sale.

**Lo que se paga, dicho en voz alta.** Dos claves con nombres distintos —`pedidos.persona_id` contra
`personas.id`, que es el caso corriente de una foránea— **no se pueden unir en v1**. Hay que hacerlas
coincidir antes, en la fuente o en su descriptor. La extensión está declarada y no construida:
`on = .a == .b`, y el día que se construya lo primero que hay que decidir es qué fila nombra cada
`.`.

**Lo que la reabre.** El primer mapping real cuyas claves no comparten nombre. No un argumento: un
`.fossil` que alguien quiera escribir y no pueda. Si llega antes que F4, la reapertura es la
referencia cualificada; si llega después, puede que sea el join externo y la nulabilidad a la vez.

---

## Reabierta y resuelta, 2026-08-10 — ver la novena enmienda de ADR-0057

La condición de arriba se cumplió por su propio criterio: `examples/users.csv` (`id,name`) contra
`examples/orders.csv` (`id,user_id,amount`), ficheros de este repositorio, con la foránea que esta ADR
llama «el caso corriente». Llegó **antes** que F4, así que la reapertura fue la que este documento
predijo — la referencia cualificada.

Y al medirlo apareció algo que esta ADR no vio: **bajo el §4, esos dos ficheros no se pueden unir ni
con la condición cualificada**, porque comparten `id` y «cualquier otro nombre compartido es un
error». No era una decisión; era una consecuencia sin medir que prohibía unir dos tablas con clave
primaria, o sea todas.

Lo que decidió la novena enmienda de ADR-0057:

- **§3 muere en su grafía y sobrevive en su intención.** `on = .k` no se puede escribir porque `.campo`
  deja de existir en el lenguaje. Inner, igualdad y clave única siguen. La razón que este §3 daba para
  rechazar la condición general —«`.` no puede significar dos filas distintas mientras no exista la
  referencia cualificada»— queda satisfecha, no contradicha.
- **§4 muere entero.** El join no aplana: produce un ámbito de filas nombradas (el modelo de Malloy),
  así que no hay `fila(izq) ⊎ fila(der)` y no hay colisión posible. Con el álgebra se va la
  prohibición del self-join, que este documento derivaba «gratis» de ella.
- **§§1, 2 y 5 no se tocan.** `join` en la primera versión, sólo `Inner`, y las dos claves con la
  misma `Primitive`.

`Op::Join { …, left_name, right_name }` ya estaba escrito para esto: «*qualify the two input streams
for collision-safe schema union*». Lo que falta sigue siendo lo que el contexto de arriba anotó — no
existe la referencia cualificada en HIR.
