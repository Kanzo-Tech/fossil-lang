# ADR 0048: El catálogo baja al checker, y una llamada deja de ser un agujero

**Date:** 2026-08-07
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Ejecuta F2 §1 de ADR-0046 (`call`, la primera de las cuatro formas). Continúa ADR-0047.
Retira `fossil-registry` como crate; ADR-0002 contaba 15 crates y ADR-0045 §4 puso `lineage` donde
estaba. Las mediciones de la pérdida silenciosa son de
`apps/docs/content/docs/characteristics/expressions.mdx`.

## Contexto

`ex:slug = clean.slug(.name)` compilaba «bien» y no escribía la columna. El 2026-08-06 el silencio se
convirtió en diagnóstico; cerrar el agujero es esta decisión, y al intentarlo aparece una pared que
no estaba en el plan.

**El checker no podía leer el catálogo.** `Primitive` bajó a una hoja en ADR-0047, pero las 56
funciones vivían en `fossil-registry`, y `fossil-registry` **dependía de `fossil-hir`** — para
materializar `FnSig<'db>`, una conveniencia que sólo usaban sus propios tests. Con esa arista, un arm
del checker que resolviera `clean.slug` era un ciclo de crates. **Durante cinco fases, la razón por
la que una llamada no se podía tipar era una dirección de dependencia**, exactamente como en
ADR-0047 la razón por la que una primitiva viajaba como texto lo era.

Y hay una segunda pared, más pequeña: el catálogo dice cómo compila cada función con el nombre de
**DuckDB** (`LoweringKind::Builtin { duckdb_name }`), mientras que el motor que ejecuta es
DataFusion. De los 21 builtins, 18 se deletrean igual en los dos y 3 no.

## Decisión

**1. El catálogo baja a `fossil-hir::stdlib`.** Es superficie del lenguaje —`stdlib.md` es una
especificación, no un detalle del backend— y el checker lo resuelve. `fossil-registry` desaparece
como crate: `lineage.rs` se queda solo y pasa a llamarse `fossil-lineage`, que es lo que es. De 24
crates a 23, y el movimiento lo fuerza este documento, que es la condición que F8 pone.

`lineage` **no** sube a `fossil-hir` aunque su contenido sea del lenguaje: sus tipos de retorno son
`fossil-run-status`, el contrato del host, y F8 disuelve eso en la carcasa. Se mueve cuando se mueva
el cable.

**2. `HirExpr::Call { func, args }`, y la cadena entera.** Forma en el HIR (bajada del
`POSTFIX_EXPR`, que es donde el parser deja tanto `clean.slug` como su aplicación) → arm en el
checker (aridad, tipos de argumento contra la firma declarada, tipo de retorno) → `Expr::Call` en MIR
→ render en `fossil-df`. `func` es el nombre de fossil (`"clean.slug"`), **nunca** el del motor.

Cuatro cosas pueden fallar y las cuatro son diagnósticos con tipo `Error`: el nombre no está
catalogado (con did-you-mean sobre el catálogo), la aridad no cuadra, un argumento no tipa, o el
tipo de un argumento no es subtipo del parámetro. Un argumento **sin** tipo no es un fallo: sin
descriptor de fuente un `.campo` no sintetiza nada, y el resto del checker ya lo deja pasar en vez de
inventárselo. F3 es lo que hace que el descriptor esté siempre.

**3. El vocabulario del motor vive con el motor.** `crates/fossil-df/src/stdlib.rs` traduce el
nombre catalogado al de DataFusion y aloja las UDF que ningún motor trae. Es la misma regla que
ADR-0047 dejó para `primitive_to_graphar`: una tabla por vocabulario, junto a quien la habla.

## Consecuencias

**Lo que se vuelve más fácil.** Las 56 funciones son alcanzables desde un programa, y 46 de ellas
corren hoy. `a_call_produces_its_column` lo comprueba por los valores —`alice`, no `Alice`— y no por
el plan. El `lsp_features` que llevaba rojo desde el 2026-08-06 vuelve a verde solo: su fixture
canónico de 200 líneas está lleno de llamadas.

**Lo que se paga.** Diez funciones no corren en este motor y están **fijadas en una lista** con su
razón: cuatro son agregados, que no son llamadas escalares y son F5; seis son UDF cuya lógica pura
existe una sola vez, dentro de los trampolines de DuckDB de `fossil-runtime`, y las dos mitades del
motor no pueden depender la una de la otra. Copiarlas aquí sería tener dos implementaciones de
`validate.email`, que es el error que ADR-0047 tardó un día en deshacer. Se mueven cuando se borre la
mitad de DuckDB — que **ya está muerta**: `fossil_runtime::execute` no tiene más llamador que su
propio test, y el codegen que emitía su SQL no existe.

**Lo que queda impreciso, dicho.** Un diagnóstico dentro de una llamada apunta a la llamada entera,
no al argumento: la arena de cuerpos guarda una entrada por valor de propiedad, así que una
subexpresión no tiene id propio. Es impreciso y lo dice; darle span propio a cada argumento es
cambiar la arena.

**Y lo que esto abre.** `Expr::Call` construido y renderizado deja `LitBool` y `BinOp` como los dos
que faltan, y ambos son `comparison` — F2 §2. El test que fija que una caída sigue siendo ruidosa se
mudó a `.age >= 18` y se mudará otra vez; el día que no quede forma, se borra en vez de debilitarse.
