# ADR 0044: Tres anillos, separados por dónde corre el código

**Date:** 2026-08-05
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** ADR-0043 (el tipo de la frontera), ADR-0042 §1 (la cámara se direcciona), ADR-0040
(fossil no publica UI). Las mediciones están en `kanzo-ui/BENCHMARKS.md`.

## Contexto

Cuatro restricciones, dadas explícitamente y no inferidas:

1. **fossil es un lenguaje de mapeo *y* un compilador de corpus**, y no haberlo decidido es parte del
   problema.
2. **Tres destinos han de funcionar:** CLI nativo, wasm en navegador, servidor MCP.
3. **Larger-than-RAM sin techo.**
4. **Se puede romper lo que sea, sin alias ni deprecación** — la misma regla que `kanzo-ui`.

**Las restricciones 2 y 3 se contradicen si ambas significan «ejecutar el motor».** wasm32 tiene ~4 GB
de espacio de direcciones y **ningún sitio donde derramar**. Un navegador no puede ser
larger-than-RAM; no es una limitación del código, es del sustrato.

Y hoy el código afirma que sí: `fossil-df-wasm` compila DataFusion a wasm, y `fossil-wasm` son 1.828
líneas de superficie de ejecución en el navegador. Mientras tanto ADR-0042 §1 decidió lo contrario
para la cámara — *«la cámara se direcciona, no se consulta»* — y esa decisión **nunca se aplicó al
motor**.

Medido, el acoplamiento actual no sigue ninguna frontera:

| crate | depende de |
|---|---|
| `fossil-cli` | `duckdb` |
| `fossil-engine` | `duckdb` |
| `fossil-mcp` | `duckdb` |
| `fossil-runtime` | `duckdb` |
| `fossil-df` | `datafusion` |
| `fossil-df-wasm` | `datafusion` |

`fossil-mcp` es tooling y arrastra un motor. `fossil-cli` también. Veintiséis crates cuyas fronteras
se dibujaron **por tema**, cuando lo que las separa es **dónde corre el código**.

## Decisión

**Tres anillos, definidos por destino de despliegue, y una regla de dependencia que se comprueba.**

### Anillo 1 — el lenguaje. No conoce ningún motor

`fossil-syntax`, `fossil-hir`, `fossil-mir`, `fossil-resolver`, `fossil-registry`, `fossil-base`,
`fossil-shex`, `fossil-descriptors-{input,output}`, `fossil-lsp`, `fossil-ide`, `fossil-ide-db`.

Analiza, tipa, diagnostica y baja a MIR. **Ninguna dependencia de `datafusion` ni de `duckdb`.** Es lo
que hace que el LSP arranque en milisegundos y compile a wasm sin arrastrar una base de datos.

`fossil-mcp` pertenece aquí y hoy no cumple.

### Anillo 2 — el compilador. Nativo, por lotes, con presupuesto

`fossil-df`, `fossil-engine`, `fossil-sinks`, `fossil-cli`, y el núcleo de maquetación que ADR-0042 §5
saca de `fossil-runtime`.

Toma MIR y fuentes, produce el corpus. **Aquí y sólo aquí vive «sin techo»**, y es alcanzable
justamente porque este anillo tiene disco: presupuesto de memoria declarado y derrame. Un anillo sin
disco no puede prometerlo, y por eso la promesa no puede vivir en los otros dos.

`fossil-runtime` se disuelve: su núcleo puro sube al anillo 2 como crate propio, y su mitad de DuckDB
desaparece con el strangler.

### Anillo 3 — el lector. wasm, y **no ejecuta programas**

Direcciona bytes contra el corpus: lee el manifiesto, resuelve qué rangos pide la cámara, entrega
Arrow al lienzo. **Ninguna dependencia de `datafusion` ni de `duckdb`.**

Esto es ADR-0042 §1 aplicado al motor y no sólo a la cámara. `fossil-df-wasm` se borra: compilar el
motor al navegador es afirmar que el navegador ejecuta programas, que es exactamente lo que ADR-0042
decidió que no. `fossil-wasm` se parte — el tokenizer y el LSP son anillo 1; lo que ejecute, fuera.

### La regla, y su guardia

> Un crate puede depender de su anillo y de los de número menor. Nunca al revés, y nunca de un motor
> que su anillo no declare.

Comprobada, no acordada — `deny.toml`:

- `duckdb` prohibido en **todo** el workspace, en cuanto el strangler cierre. Hoy en rojo, y ése es el
  criterio de terminado que le ha faltado desde junio.
- `datafusion` prohibido en los anillos 1 y 3.

Un ADR sin guardia es una intención. `kanzo-ui` borró ocho composites y quedó *terminado* porque
`index.test.ts` afirma que no están; aquí hay 46 ADRs y ninguna afirmación ejecutable, y por eso
ADR-0042 puede listar `viewport` como «fuera» y `viewport` sigue ahí.

## La consecuencia cara, y no hay forma de evitarla

**Con «sin techo», Louvain sobre el grafo entero no sobrevive.** No por lento: por O(V) en heap. A mil
millones de vértices el array `community` son 4 GB antes de tocar una arista, y ninguna de las
constantes que ADR-0043 ataca cambia el orden.

Dos salidas, y hay que elegir explícitamente:

1. **Louvain externo**, con el estado de vértice en disco — GraphChi, X-Stream. Es un proyecto en sí.
2. **La jerarquía deja de ser obligatoria**, calculada sobre una muestra o sobre el grafo ya contraído.

**Se recomienda la 2, y no por pereza.** La 1 construye infraestructura para sostener algo cuyo valor
no está demostrado: lo medido el 2026-08-04 dice que **los tramos contiguos que abaratan una ventana
los produce la curva de Morton, no las comunidades** — `cluster_id` tiene mediana 1 y `community` son
ocho grupos rotos en 5.461 tramos cada uno. Se pagan O(V) y 132 de los 139 segundos por una estructura
cuya contribución a la lectura nadie ha medido.

El orden correcto es **medir primero si la jerarquía sirve para el LOD**, y sólo entonces decidir
cuánto vale sostenerla. Esa medición no existe y es el prerrequisito de esta decisión.

## Consecuencias

**Lo que se borra** (regla 4, sin alias): `fossil-df-wasm` (436), la mitad ejecutora de `fossil-wasm`,
`fossil-runtime` como crate, y `duckdb` de los cuatro `Cargo.toml`.

**Lo que se mueve:** el núcleo puro de `layout.rs` al anillo 2; `fossil-mcp` al anillo 1, sin motor.

**Lo que deja de hacerse:** optimizar constantes dentro de `enrich_layout`. Intentado el 2026-08-04 y
regresó siete gigabytes; con «sin techo» como objetivo, pasar de 53 a 30 B/arista no cambia la
categoría del problema, sólo mueve el punto donde revienta.

**Lo que este ADR no resuelve.** Dónde están los ~14 GiB de W0b — localizados antes de la maquetación,
en el camino de escritura, pero **no atribuidos**. El presupuesto del anillo 2 los acota sin
explicarlos, y explicarlos sigue pendiente.

**Y lo incómodo.** Esta separación estaba implícita desde que se eligió DataFusion *porque compila a
wasm*; lo que no se hizo fue decir qué corre dónde, y por eso el motor acabó compilado al navegador y
el tooling acabó dependiendo de una base de datos. El diseño no se torció por una decisión mala: se
torció por una decisión no tomada.
