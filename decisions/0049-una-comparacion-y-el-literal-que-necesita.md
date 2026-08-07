# ADR 0049: Una comparación, y el literal que hacía falta para escribirla

**Date:** 2026-08-07
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Ejecuta F2 §2 de ADR-0046 (`comparison`). Continúa ADR-0048. Las reglas son `type-system.md`
§4.7 (T-Comp, T-And). El agujero del literal entero se midió el 2026-08-07 con
`crates/fossil-hir/src/lower.rs` en el árbol.

## Contexto

`comparison` enciende el filtro del pipeline y la condición del ternario, y es la segunda de las
cuatro formas. Al escribir su primer test aparece que **no se puede escribir**: `.age >= 18` necesita
un `18`, y un literal entero no era una forma del HIR.

Y no era un vacío conocido, era **un segundo agujero silencioso**, distinto del que se cerró el
2026-08-06. Aquel arreglo hizo ruidosas las *clases de nodo* que el lowering no lee; un entero llega
en un `LITERAL_EXPR`, que es una clase que sí lee, por un brazo que devolvía `None` sin decir nada.
Medido: `ex:n = 42` producía **una propiedad y cero diagnósticos**. El mismo modo de fallo, en el
mismo punto ciego, escondido detrás del arreglo del día anterior.

## Decisión

**1. `HirExpr::IntLit(i64)`, y el flotante lo dice.** Un `FLOAT` es un diagnóstico, no una caída
silenciosa: `Expr` es `Hash + Eq` porque Salsa lo interna y `f64` no es ninguna de las dos, así que
llevarlo necesita una decisión de representación —no un cast que nadie declaró—. Y el brazo de
`LITERAL_EXPR` deja de devolver `None` en silencio para cualquier otra forma.

**2. `HirExpr::BinOp { op, lhs, rhs }` cubre las seis comparaciones más `and`/`or`, y la aritmética
no.** `+`, `-`, `*`, `/`, `%` producen el mismo nodo CST y bajan a un diagnóstico, porque **MIR no
tiene operador aritmético**. Meterlas en el HIR sería poner una expresión que nada aguas abajo puede
ejecutar, que es la forma exacta del problema que esta fase cierra. Una forma cada vez, y cada una
entera.

**3. `CmpOp` baja a `fossil-hir`.** Estaba en `fossil-mir`, y un operador que el parser lee y el
checker tipa no puede definirse aguas abajo de ambos. Es el mismo movimiento que ADR-0048 hizo con el
catálogo y ADR-0047 con el retículo: **la tercera vez que un tipo del lenguaje vivía debajo de quien
lo necesita**. `fossil-mir` y los backends nombran el de `fossil-hir`; no hay reexportación.

**4. Un operando sin tipo no es un error.** Sin descriptor de fuente un `.campo` no sintetiza nada, y
el resultado es `Bool` igual: lo dice el operador, y los operandos no pueden cambiarlo. Lo que sí se
rechaza es lo que un mapping se equivoca de verdad —comparar una columna de texto con un número— y
eso sólo se puede ver cuando el descriptor está, que es F3.

## Consecuencias

**Lo que se vuelve más fácil.** `render` en `fossil-df` **es total**: no queda un `unimplemented!()`
que alcanzar, que es lo que hace que una propiedad que tipa sea una propiedad que corre. `Expr::BinOp`
y `Expr::LitBool` dejan de ser variantes que nada construye. Y el predicado que F5 necesita para
`where(...)` ya tiene su forma, su regla de tipos y su render.

**Lo que se paga.** La aritmética sigue fuera y ahora es la única forma de expresión que el parser
construye y el HIR rechaza por falta de destino, no por falta de decisión. El test que fija que una
caída sigue siendo ruidosa se mudó a `.id * 2` — la tercera mudanza, y se mudará otra vez.

**Lo incómodo, dicho.** Dos agujeros silenciosos en dos días, los dos en el mismo brazo, el segundo
tapado por el arreglo del primero. La lección no es «faltaba un test»: es que **un `None` en un
lowering es una decisión de no escribir un dato**, y hasta que cada uno de ellos diga por qué, el
siguiente está escondido en el mismo sitio.
