# ADR 0050: El condicional, y que la tubería en posición de valor es azúcar

**Date:** 2026-08-07
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Ejecuta F2 §3 y §4 de ADR-0046. Continúa ADR-0048 y ADR-0049. Las reglas son
`type-system.md` §4.6 (T-Pipe) y §4.8 (T-Tern).

## Contexto

Quedaban dos de las cuatro formas. El condicional es directo: el parser ya construye
`TERNARY_EXPR` y `type-system.md` §4.8 ya dice qué tipa. La tubería no lo es, y la razón es que
**`|>` significa dos cosas distintas** según dónde aparezca:

- `.name |> clean.trim()` — un **valor**, la propiedad de un vértice.
- `adultos := users |> where(.edad >= 18)` — una **relación**, un binding de fuente.

La segunda es el pipeline del que habla F5, y F5 arranca con una decisión abierta que no es del
compilador: **si `join` entra en la primera versión**. La primera no espera a nada.

## Decisión

**1. `HirExpr::Ternary { cond, then, otherwise }`, con las dos ramas del mismo tipo.** Sin coerción
implícita: dos ramas de tipos distintos es el error, no un ensanchamiento que nadie pidió. Una
columna cuyo tipo depende de la fila es una columna que ninguna forma puede comprobar, y comprobar
la forma es la tesis del proyecto. Baja a `Expr::Ternary` y se renderiza como un `CASE` de dos
brazos — `otherwise` siempre está, así que ninguna fila cae a NULL por la puerta de atrás.

**2. `|>` en posición de valor es azúcar, no una forma.** `type-system.md` §4.6 dice que la tubería
pasa el lado izquierdo como **primer argumento**; con `call` ya en el árbol eso es literalmente una
llamada con un argumento más. `lower_pipeline` desazucara y el checker, MIR y el backend ven la
llamada que ya conocen. **No hay nodo de tubería en el HIR**, y no debe haberlo: un nodo que sólo
sabe reescribirse a otro es un nodo que hay que mantener dos veces.

**3. La tubería de fuente no se toca aquí.** Es una relación, no un valor: necesita que un binding
de fuente pueda ser algo que no sea `io.*(...)`, que `Op::Filter` sea alcanzable desde el código
fuente, y que `where`/`select`/`join` tengan tipos. Es F5 entera, y **empieza por una decisión de
producto que hay que tomar antes de escribir una línea**.

## Consecuencias

**Lo que se vuelve más fácil.** Las cuatro formas de ADR-0046 §2 están, cada una entera y con su
valor comprobado end-to-end. `render` en `fossil-df` sigue siendo total. Y el condicional es lo que
hace que la sensibilidad como tipo (F4 §3) sea escribible sin nulos: `sensible ? anon.hmac(.x) : .x`
es una expresión con un solo tipo.

**Lo que se paga.** El azúcar tiene un coste diagnóstico: un error dentro de `.name |> clean.trim()`
habla de `clean.trim`, porque para cuando el checker mira ya no hay tubería. Es correcto y es
impreciso, y prefiero decirlo a fingir que el desazucarado es transparente.

**Lo que queda abierto, y es de Angel.** Si `join` entra en la primera versión de F5. `where` y
`select` puede que basten un tiempo, y son mucho más baratos de tipar: `join` es lo que convierte al
checker en el trabajo principal.
