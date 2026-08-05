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

| crate | depende de | cómo |
|---|---|---|
| `fossil-engine` | `duckdb` | normal |
| `fossil-mcp` | `duckdb` | normal |
| `fossil-runtime` | `duckdb` | normal |
| `fossil-cli` | `duckdb` | **sólo `[dev-dependencies]`** (`Cargo.toml:38`), para leer el Parquet de vuelta en los tests |
| `fossil-df` | `datafusion` | normal |
| `fossil-df-wasm` | `datafusion` | normal |

`fossil-mcp` es tooling y arrastra un motor. Veintiséis crates cuyas fronteras se dibujaron **por
tema**, cuando lo que las separa es **dónde corre el código**.

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

**Corregido el 2026-08-05: esa partición ya había ocurrido.** A `fossil-wasm` no le queda superficie
de ejecución que sacar — `compile_file` no existe en `src/` (sólo lo nombran una fila de la tabla del
encabezado y los tests JS), sus dependencias no incluyen ni `duckdb`, ni `datafusion`, ni `fossil-df`,
ni `fossil-runtime`, ni `fossil-engine`, y su único uso del backend, `lower_to_mir_pg`
(`lib.rs:737`), **descarta el resultado** con `let _ =` y sólo drena el acumulador de diagnósticos.
El crate es lenguaje y LSP, que es donde este ADR quería dejarlo.

### La regla, y su guardia

> Un crate puede depender de su anillo y de los de número menor. Nunca al revés, y nunca de un motor
> que su anillo no declare.

Comprobada, no acordada — `deny.toml`:

- `duckdb` prohibido en **todo** el workspace, en cuanto el strangler cierre. Hoy en rojo, y ése es el
  criterio de terminado que le ha faltado desde junio.
- `datafusion` prohibido en los anillos 1 y 3.

### Corregido el 2026-08-05 — la arista que sostenía este párrafo no existe

El párrafo original decía que el trinquete con `wrappers` sobre los cuatro dependientes directos de
`duckdb` no pasaba, y que lo que destapó al primer intento era que `fossil-wasm` —el anillo del
lector— *«alcanza el motor por transitividad, vía `fossil-ide → fossil-engine`»*.

**Esa arista no existe, y la que sí existe va en el sentido contrario:** `fossil-engine → fossil-ide`
(`fossil-engine/Cargo.toml:23`, para `providers` y `source_refs`). El grafo se puede consultar, y
`cargo tree -e normal -i duckdb --workspace` devuelve entera esto:

    duckdb v1.10502.0
    ├── fossil-engine → fossil-cli
    ├── fossil-mcp
    └── fossil-runtime → fossil-engine, fossil-mcp

Ni `fossil-wasm` ni `fossil-ide` aparecen, ni siquiera con dev-dependencies incluidas. Lo que
cargo-deny rechazó como `unmatched wrapper` no era transitividad: era que esos nombres **no son
dependientes en absoluto**, y una entrada de `wrappers` que nombra a un no-dependiente es un error de
configuración, no un hallazgo. El cuarto nombre, `fossil-cli`, está en la misma situación por otra
razón: declara `duckdb` sólo en `[dev-dependencies]`.

**El estado real es mejor que el que este ADR describía.** Los dependientes normales de `duckdb` son
**tres** —`engine`, `mcp`, `runtime`— y el anillo del lector ya está limpio de los dos motores. La
guardia tampoco está hoy en el árbol: `ed8a732` la sacó de `deny.toml` y la dejó como prosa aquí
dentro. Con tres nombres en vez de seis, **activarla cuesta menos de lo que este documento creía**, y
deja de ser «la primera tarea del strangler» para ser un trinquete que fija lo ya conseguido.

La lección, y es la que vale más que la corrección: **el grafo de dependencias se puede consultar y
no se consultó**, y el comentario de `fossil-wasm/Cargo.toml` —*«NOT pull fossil-runtime or any
native UDF crate (Pitfall 3)»*— se leyó como si fuera una restricción incumplida cuando era una
intención cumplida. Ver ADR-0045 §7.

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
cuánto vale sostenerla.

### Medido el 2026-08-05, y la recomendación de arriba era equivocada

Un supernodo por comunidad sólo resume con honestidad si la comunidad es **compacta en el espacio**.
Comparado contra la alternativa que no cuesta nada —bins contiguos de la curva, mismo número de
grupos— sobre el corpus de cinco millones, lienzo de 5.289.638 de ancho:

| partición | grupos | radio medio | **p90** |
|---|---|---|---|
| `cluster_id` | 15.310 | 4.108 | **20** |
| bin de Morton | 15.291 | 44.654 | **72.722** |

**El 90 % de las comunidades cabe en un radio de veinte unidades.** Son un punto. Un bin de la curva
del mismo tamaño ocupa 72.722 — tres mil seiscientas veces más — porque el orden Z da saltos largos
al cruzar bits altos, que es su debilidad conocida frente a Hilbert.

Así que **la jerarquía sí gana su sitio, y la salida 2 queda descartada.** Las dos afirmaciones
conviven sin contradicción y hay que leerlas juntas: para **traer** una ventana, lo que abarata es la
curva y las comunidades no aportan (ADR-0043); para **agregar** una vista alejada, las comunidades
son fieles y la curva no sirve. Son dos preguntas distintas sobre la misma columna, y hasta hoy sólo
se había medido una.

Consecuencia: **Louvain tiene que sobrevivir**, y por tanto «sin techo» exige la salida 1 —estado de
vértice fuera del heap, GraphChi/X-Stream— que es el proyecto que esta sección esperaba poder evitar.
La medición cerró la puerta barata.

## Consecuencias

**Lo que se borra** (regla 4, sin alias): `fossil-df-wasm` (436), `fossil-runtime` como crate, y
`duckdb` de los **tres** `Cargo.toml` que lo declaran como dependencia normal —`engine`, `mcp`,
`runtime`— más el `[dev-dependencies]` de `fossil-cli`. La mitad ejecutora de `fossil-wasm` ya no
está: ver la corrección del anillo 3.

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
