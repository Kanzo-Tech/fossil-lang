# ADR 0043: El tipo de la frontera es lo que obliga a materializar

**Date:** 2026-08-04
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** ADR-0042 (la cámara se direcciona), cuyo §5 pedía sacar el núcleo puro; `kanzo-ui/BENCHMARKS.md`
y `kanzo-ui/.planning/GRAPH-ROADMAP.md`, de donde sale cada número.

## Contexto

Escribir un corpus de diez millones de vértices pica **17,0 GiB** para producir **713 MB**. Veintitrés
veces la salida, residente. Cuatro hipótesis sobre dónde iba esa memoria resultaron falsas, cada una
descartada por medición y no por argumento:

| candidato | por qué parecía | medición que lo descartó |
|---|---|---|
| los `HashMap` por comunidad en `contract` | `vec![HashMap::new(); k]`, k en millones | el núcleo entero son 53 B/arista |
| el núcleo de maquetación | ADR-0042 lo predijo explícitamente | 3,94 GiB aislado, de 17 |
| DuckDB durante la maquetación | sin límite por defecto | acotado a 2 GB: total igual, derrame **0 B** |
| el generador de Node | tenía 12 GB de heap | `fossil run` solo llega al mismo pico |

Lo que sí dice la instrumentación (`FOSSIL_LAYOUT_PROBE=1`): **el RSS ya es 14,61 GiB cuando
`enrich_layout` ejecuta su primera sentencia**, y toda la pasada añade 0,13 GiB netos. Y leer las 71M
aristas a un `Vec` da **−3,12 GiB**: se libera más de lo que se reserva.

Y dos hechos verificados sobre el artefacto:

- **`by_source.parquet` tiene cero desórdenes por `src_dense`** (71.024.690 filas). Es CSR en disco.
- **`by_target.parquet` tiene cero desórdenes por `dst_dense`.** Es CSC en disco.

## La causa, que es de tipos y no de algoritmo

`Weighted::from_edges(vertex_count: u32, edges: &[(u32, u32)])` toma **una bolsa desordenada**. De ahí
salen, por necesidad, tres cosas:

1. la bolsa hay que materializarla para pasarla — `Vec<(u32,u32)>`, 568 MB;
2. hay que contar grados en una pasada previa y hacer prefix-sum;
3. hay que **dispersar** con un `cursor` clonado (80 MB) haciendo escrituras aleatorias sobre un array
   de 568 MB — hostil a la caché además de caro.

Ninguna de las tres la pide el algoritmo. `local_moving` recorre `for v in 0..n { neighbours(v) }`, que
es **un escaneo secuencial del CSR**; lo único de acceso aleatorio es `community[u]`, que es O(n) —
unos 200 MB de estado a diez millones, no 2,9 GB.

**El fichero lleva ordenado desde que se escribió y el tipo del parámetro dice que no lo sabe.**

Lo mismo, una altura más arriba: las mismas aristas atraviesan **seis representaciones** en una
generación — CSV, Arrow (DataFusion), Parquet en disco, Arrow (DuckDB), `Vec<(u32,u32)>`, arrays CSR.
Cada frontera es una copia, y cada frontera existe porque el tipo cambia al cruzarla.

## Decisión

**El tipo de la frontera es un CSR ordenado, no una bolsa de pares.** Y donde el motor ya tiene ese
tipo — la `ListArray` de Arrow *es* offsets + valores, que *es* CSR — la frontera desaparece en vez de
abaratarse.

De ahí salen seis etapas. Cada una está justificada por una medición y **cada una es útil sola**: nada
depende de que la siguiente ocurra.

### Etapa 1 — leer el CSR que ya existe

`from_edges(&[(u32,u32)])` → `from_csr(offsets, targets)`. Los offsets salen de un `GROUP BY
src_dense` (10M × 8 = 80 MB); los targets son una columna que se lee directa. Sin bolsa, sin contar,
sin dispersar.

Y los pesos se vuelven un `enum`: en el nivel 0 **todos valen 1,0** y ocupan 16 de los 53 B/arista
medidos. Sólo la contracción produce la variante con pesos.

| | ahora | después |
|---|---|---|
| `Vec<(u32,u32)>` | 568 MB | — |
| `cursor` | 80 MB | — |
| `weights` nivel 0 | 1.136 MB | — |
| `targets` | 568 MB | 568 MB |

**Diff:** `from_edges` −60, `from_csr` + enum de pesos +95, el bucle de lectura en `enrich_layout` −25,
dos lecturas de columna +35. **Neto ≈ +45 LOC, ≈ −1,7 GB.**

**Sostenido por:** `crates/fossil-runtime/tests/layout_renumber.rs` (279 líneas, tres casos incluido el
camino feliz) debe pasar sin tocarse. Ésa es la definición de paridad.

#### Intentada el 2026-08-04, y refutada por la medición

Se implementó entera: `Weights::{Unit,Stored}`, `Weighted::from_sorted` con el merge de los dos
flujos, `community_hierarchy_sorted`, el campo `self_edge_csc`, y `enrich_layout` leyendo las dos
sentencias en vez de construir la bolsa. Compiló y **los tres tests de paridad pasaron sin tocarse**.

Y el pico subió:

| | antes | con el merge |
|---|---|---|
| RSS al empezar `enrich_layout` | 14,61 GiB | **21,54 GiB** |
| pico del proceso | 17,0 GiB | **24,4 GiB** |

**`query_map` de `duckdb-rs` no streamea: materializa el resultado completo.** Así que las dos
sentencias abiertas a la vez son dos conjuntos de resultados residentes — más el CSR que se está
construyendo — donde antes había una sola `Vec`. Se cambió una copia por dos.

El razonamiento de esta etapa no cae: el fichero sigue ordenado, el algoritmo sigue siendo
secuencial, y las tres asignaciones que el tipo obliga siguen siendo innecesarias. Lo que cae es la
suposición no comprobada de que el *driver* streamea. **Leer el CSR que ya existe requiere leer
Parquet directamente** —`parquet`/`arrow` por row group, sin DuckDB en medio— lo que hace que esta
etapa dependa de la 2 y la 3 en lugar de ser independiente como se afirmaba arriba.

Cambio revertido; el árbol queda como estaba. Lo que sobrevive es el número, que es lo que hacía
falta para no volver a intentarlo igual.

### Etapa 2 — el núcleo puro sale del crate que posee DuckDB

Lo que pedía ADR-0042 §5, ahora con la razón medida: `community_hierarchy`, `cluster_layout`,
`local_moving`, los Morton y `Weighted` no tienen I/O y están presos en `fossil-runtime` porque ese
crate depende de DuckDB, y por eso no son wasm-limpios.

`layout.rs` son 1.486 líneas, de las que `enrich_layout` (258) y el `Probe` (~100) son la orquestación
acoplada al motor. El resto es núcleo.

**Diff:** crate nuevo `fossil-layout` ≈ +1.050 (movidas), `fossil-runtime/layout.rs` −1.050,
`Cargo.toml` +12. **Neto ≈ +12 LOC.** No ahorra memoria; desbloquea las etapas 3 y 6.

### Etapa 3 — un motor, con guardia ejecutable

DuckDB fue el ejecutor original (2026-05-15); DataFusion llegó a reemplazarlo (2026-06-09) por pasos
con nombre, el último `compila a wasm32`. El código ya lo dice: *«DuckDB — the one remaining
native-runtime use on this path»*. Lo que queda son **8 `execute_batch` y 1 `appender`**.

Cuatro crates dependen hoy de `duckdb`: `fossil-cli`, `fossil-engine`, `fossil-mcp`, `fossil-runtime`.

| | LOC |
|---|---|
| `enrich_layout` reescrito contra DataFusion | 258 → ≈ 180 |
| `graph_exec.rs` (la costura `DuckExecutor`) borrado | −107 |
| `udf.rs` reescrito como `ScalarUDF` de DataFusion | 368 → ≈ 300 |
| `enrich_written_layout` en `fossil-engine` | 78 → ≈ 55 |
| `deny.toml`: prohibir `duckdb` en el árbol de `run` | +15 |

**Neto ≈ −290 LOC.** El guard es lo que hace que la migración termine: hoy nadie se entera de que
falta porque los dos caminos funcionan.

### Etapa 4 — presupuesto de memoria en la frontera

`crates/fossil-df/src/lib.rs:857` es `SessionContext::new()`: pool ilimitado, sin `DiskManager`. En
todo el workspace no aparece `RuntimeEnvBuilder` ni `DiskManager`. **Larger-than-RAM no está sin
probar en este camino: es imposible por construcción.**

Y la ironía medida: `apply_resource_limits` existe y está aplicado a la conexión DuckDB de la
maquetación, que gasta 0,13 GiB netos.

| | LOC |
|---|---|
| `RuntimeEnvBuilder` con pool acotado + `DiskManager` | +35 |
| presupuesto como entrada del `run` (CLI + API) | +45 |
| `apply_resource_limits` (DuckDB) borrado | −40 |
| test: corpus pequeño, límite ridículo, exigir derrame | +60 |

**Neto ≈ +100 LOC**, y convierte un OOM en lentitud o en un error legible.

### Etapa 5 — la superficie deja de ser el contrato

ADR-0042 lleva 17 verbos a 6. `operations/mod.rs` declara 17 hoy. El contrato es el fichero —Parquet
ordenado en Morton, CSR y CSC, manifiesto— y eso es lo que hace que un navegador, un cuaderno y un
servicio sean ciudadanos iguales. Hoy se leyó ese corpus desde tres lenguajes que no saben que fossil
existe.

| | LOC |
|---|---|
| `operations/viewport.rs` borrado | −181 |
| `exec.rs` (1.894) sin el camino de viewport | ≈ −500 |
| `operations/graphrag.rs`, parte de `discovery.rs` | ≈ −120 |
| `tests/graph_verbs.rs` (406) recortado | ≈ −120 |

**Neto ≈ −920 LOC.**

### Etapa 6 — la maquetación deja de ser una segunda pasada

Hoy: escribir Parquet → releerlo → calcular → reescribir en sitio. Con el núcleo sobre Arrow
(etapa 2) y un motor (etapa 3), la colocación es parte del plan de escritura.

**Diff estimado ≈ −250 LOC**, con el margen de error más ancho del documento — es la única etapa que
es rediseño y no reescritura, y no debe empezarse antes que las cinco anteriores.


## Patrones y arquitecturas de referencia

Cada etapa es una instancia de algo que ya tiene nombre y de un sistema que ya lo hace. Se nombran
aquí porque un patrón con referencia se puede *comprobar* contra ella; uno sin referencia es una
opinión con vocabulario.

### Etapa 1 — *edge-centric streaming* (X-Stream, GraphChi)

La pregunta «¿por qué materializamos?» tiene una respuesta publicada. **X-Stream** (Roy et al.) se
construye entera sobre esta observación: en un grafo, recorrer las **aristas en secuencia** mientras
se mantiene sólo el estado **por vértice** en memoria bate al acceso aleatorio sobre una adyacencia
residente, porque el ancho de banda secuencial es órdenes de magnitud mayor que el aleatorio.
**GraphChi** (Kyrola et al.) llega a lo mismo por otro camino — *parallel sliding windows* — y ambos
procesan grafos que no caben en RAM en una máquina.

Aquí encaja exacto: `local_moving` recorre `for v in 0..n { neighbours(v) }`, que es un escaneo
secuencial; el estado aleatorio es `community[u]`, O(n), ~200 MB a diez millones. **El corpus ya está
en el orden que estos sistemas se esfuerzan en construir** — `by_source.parquet`, cero desórdenes.

Y el mecanismo que lo hace irreversible es de tipos: **«parse, don't validate»**. `from_csr(offsets,
targets)` no puede necesitar la pasada de conteo porque su firma ya afirma el orden; `&[(u32,u32)]`
obliga a reconstruirlo porque no afirma nada. El invariante vive en el tipo, no en un comentario.

### Etapa 2 — puertos y adaptadores (*hexagonal*)

Lo que ADR-0042 §5 pedía sin nombrarlo. El núcleo —`community_hierarchy`, `cluster_layout`, Morton—
es el dominio: sin I/O, sin RNG, sin base de datos. DuckDB y DataFusion son adaptadores. Hoy el
dominio vive dentro de un adaptador, que es la inversión exacta que el patrón existe para evitar, y
tiene una consecuencia medible y no estética: **no es wasm-limpio**, que era la razón de elegir
DataFusion.

### Etapa 3 — *strangler fig*, y por qué se atascó

La migración DuckDB → DataFusion es un **strangler fig** de manual (Fowler): el sistema nuevo crece
alrededor del viejo por rodajas con nombre — `arranque vertex-only`, `paridad DuckDB`, `paridad de
formatos`, `compila a wasm32` — hasta estrangularlo.

El patrón tiene un modo de fallo conocido y es el que ocurrió: **un strangler sin fecha de defunción
no termina**, porque mientras los dos caminos funcionan nada obliga al último paso. Lleva al 90%
desde junio. Lo que lo cierra no es esfuerzo sino un **architecture fitness function** (Ford, Parsons
& Kua): `deny.toml` prohibiendo `duckdb` en el árbol de `run`. Hoy está en rojo; el día que esté en
verde, la migración terminó y nadie tiene que acordarse.

Es el mismo mecanismo que `kanzo-ui` ya usa: `index.test.ts` afirma que los ocho composites borrados
*no* están en el barrel. Ahí «dos maneras de hacer X» es un test en rojo; aquí todavía no.

### Etapa 4 — protocolo de reserva y derrame

No es un flag, es un protocolo, y los tres motores de referencia lo implementan igual: **DataFusion**
(`MemoryPool`, `GreedyMemoryPool` / `FairSpillPool`, `DiskManager`), **DuckDB** (`memory_limit` +
`temp_directory`, hash join y sort out-of-core) y **Velox** (`MemoryPool` + arbitraje con derrame).
La forma es: *el operador reserva antes de asignar y sabe qué hacer si le dicen que no*.

«Intentar y morir» no es un modo que exista en ninguno de los tres. `SessionContext::new()` lo es por
defecto.

### Etapa 5 — el artefacto es la API

**Iceberg**, **Delta Lake** y **Lance** son la referencia de lo que ya tenéis a medio declarar:
manifiesto más ficheros, y *muchos* motores leyendo la misma tabla sin que ninguno sea el contrato.
Y para el lado de lectura por rangos, el linaje es **Cloud-Optimized GeoTIFF → GeoParquet →
PMTiles**: fichero único, directorio embebido, orden espacial, y el cliente pide **rangos de bytes
por HTTP** sin servidor.

PMTiles importa además porque resuelve la pregunta abierta nº 1 del roadmap —169 rangos por ventana
son 169 peticiones— con un diseño ya desplegado a escala planetaria. Es el sitio del que copiar el
esquema de teselas en vez de inventarlo.

### Etapa 6 — la maquetación como operador físico

El modelo **Volcano** (Graefe) y su descendencia vectorizada — **MonetDB/X100**, **DuckDB**,
**Velox**, DataFusion — dicen que una transformación es un **operador del plan**, no una pasada
posterior sobre el artefacto. Hoy la maquetación escribe Parquet, lo relee y lo reescribe: es una
pasada fuera del plan, y por eso necesita su propia conexión, su propia representación y su propio
presupuesto.

Como `ExecutionPlan` hereda las tres del motor. Y **morsel-driven parallelism** (Leis et al., HyPer)
es lo que hace que `local_moving` sea paralelizable sin inventar un planificador: cada morsel es un
rango de vértices y el estado compartido es un array O(n).

### Lo que deliberadamente no se adopta

**Un framework de grafos** (Ligra, GBBS, cuGraph). Resuelven el problema de otro: cargar un grafo
arbitrario y correr muchos algoritmos sobre él. Aquí hay **un** algoritmo, sobre un corpus **que este
sistema acaba de escribir y cuyo orden controla**. La referencia útil de ese mundo no es el
framework: es la representación — CSR liso a ~8 B/arista, y **WebGraph** (Boldi–Vigna) como techo de
lo que la compresión de adyacencias da si algún día hiciera falta.

**Repartir en varias máquinas.** Con 53 B/arista y un pico de 3,94 GiB a 69,5M aristas, una máquina
llega mucho más lejos que la escala que se ha medido. Distribuir antes de agotar `mmap` y el derrame
es cambiar un problema medido por uno mayor sin medir.

## Consecuencias

| etapa | LOC neto | memoria | depende de |
|---|---|---|---|
| 1 · CSR desde el artefacto | **+45** | −1,7 GB | — |
| 2 · núcleo puro fuera | +12 | — | — |
| 3 · un motor + guardia | −290 | — | 2 |
| 4 · presupuesto | +100 | acota el resto | 3 |
| 5 · el fichero es el contrato | −920 | — | — |
| 6 · maquetación en la escritura | −250 | los ~14 GiB de W0b | 1,2,3 |
| **total** | **≈ −1.300** | | |

**Las etapas 1, 2 y 5 no dependen de nada** y pueden ir en paralelo. La 1 es la que más devuelve por
línea escrita: cuarenta y cinco líneas por un gigabyte y siete.

**Lo que este ADR no resuelve.** Dónde están los ~14 GiB de W0b. Están *localizados* —antes de la
maquetación, en el camino de escritura de DataFusion— pero no *atribuidos*, y la etapa 4 los acota sin
explicarlos. Atribuirlos es la continuación honesta de este documento y no está hecha.

**Y lo incómodo, que es lo que hace útil un ADR.** Tres de las cuatro hipótesis descartadas arriba las
propuse yo con confianza en la misma tarde. La etapa 6 lleva una estimación de LOC que no me creo del
todo. Y el número que abre este documento —23× la salida, residente— se descubrió por accidente
midiendo otra cosa, lo que significa que nadie lo estaba vigilando: el test de la etapa 4 es lo único
aquí que impide que vuelva a pasar.
