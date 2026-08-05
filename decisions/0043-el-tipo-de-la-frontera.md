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

Lo que sí dice la instrumentación (`FOSSIL_MEM_PROBE=1`): **el RSS ya es 14,61 GiB cuando
`enrich_layout` ejecuta su primera sentencia**, y toda la pasada añade 0,13 GiB netos. Y leer las 71M
aristas a un `Vec` da **−3,12 GiB**: se libera más de lo que se reserva.

Esos catorce quedaron atribuidos el 2026-08-05 y no son ninguna de las cinco cosas de arriba: son
memoria de operador de DataFusion que nunca se acotó — la etapa 4, y el único sitio de este documento
donde la medición ya dio un número de después.

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
suposición no comprobada de que el *driver* streamea.

#### Y la refutación se pasó de pesimista

Escrita, decía que hacía falta leer Parquet directamente y que por tanto esta etapa dependía de la 2
y la 3. **Comprobado en la fuente de `duckdb-rs` 1.10502.0, no es así:**

| | qué hace | |
|---|---|---|
| `Statement::query_map` | `execute()` — materializa el resultado entero | lo que se usó, y lo que regresó |
| `Statement::stream_arrow(params, schema)` | `execute_streaming()` + `ArrowStream` | perezoso de verdad |

`ArrowStream` implementa `Iterator<Item = RecordBatch>` con un `stream_step()` por llamada
(`src/arrow_batch.rs:61-67`). Un batch cada vez, no el resultado.

Y lo que lo hace la elección correcta y no un parche: **el item es `RecordBatch`**. La API perezosa
entrega exactamente la frontera que este ADR defiende —Arrow, offsets y valores— en vez de filas
convertidas a tuplas de Rust una a una. El `from_sorted` que se escribió toma
`impl Iterator<Item = (u32, u32)>`; sobre `stream_arrow` toma columnas `UInt32Array` por batch, que
es menos código y ninguna conversión por fila.

Así que la etapa 1 sigue siendo independiente, y el cambio pendiente es de una línea de API y del
bucle que lee las dos columnas. Lo único que este intento demostró es que **el criterio de terminado
estaba mal**: tres tests de paridad en verde no vieron una regresión de siete gigabytes, porque
comprobaban que el resultado fuera correcto y el coste era el objetivo entero. El siguiente intento
lleva la medición del `Probe` en el criterio, no sólo la paridad.

Cambio revertido; el árbol queda como estaba. Lo que sobrevive son los dos números y el nombre de la
función que faltaba.

#### Rehecha el 2026-08-05 sobre `stream_arrow`, y la predicción se cumple

`Statement::stream_arrow` era, en efecto, lo que faltaba. Con él la etapa entera cabe donde el ADR
decía que cabía, y las tres cosas que el tipo obligaba —la bolsa, el conteo de grados y la dispersión
por el cursor— desaparecen sin sustituto.

| a diez millones | antes | con el CSR |
|---|---|---|
| RSS al empezar `enrich_layout` | 7,05 GiB | 6,89 GiB |
| leer las aristas | −0,43 GiB · 2,3 s | +0,29 GiB · 0,9 s |
| `community_hierarchy` | +2,09 GiB · 135,0 s | **+0,08 GiB** · 168,0 s |
| pico del proceso | 10,10 GiB | **8,39 GiB** |

**−1,71 GiB de pico, contra los ≈ −1,7 GB predichos**, y el corpus sale idéntico byte a byte: 87
ficheros, los 87 md5 iguales a los del binario anterior sobre el mismo CSV. Los tres tests de paridad
pasan **sin tocarse**, que era la otra mitad del criterio.

Lo que dice la fila de `community_hierarchy`: antes el paso costaba +2,09 GiB porque construía el CSR
dentro; ahora el CSR ya existe y el paso cuesta +0,08 GiB. La memoria no se movió de sitio, dejó de
pedirse.

Tres cosas que este intento corrige del anterior, y ninguna estaba en el plan:

- **No hace falta el campo `self_edge_csc`.** Añadirlo obliga a editar el literal de struct de
  `tests/layout_renumber.rs`, que es precisamente el fichero que define la paridad. La otra
  orientación no es una entrada nueva: el llamante ya enumera *todos* los ficheros de adyacencia con
  el extremo por el que están ordenados, porque la renumeración reescribe ambos. Se encuentra ahí.
- **`from_edges` no se borra.** El coste de una bolsa lo sigue pagando quien tiene una bolsa —los
  tests y `examples/layout_memory.rs`, que existe para medir justamente eso—. Lo que cambia es que el
  camino de escritura ya no tiene ninguna.
- **`Weighted` guarda una lista de lados, no un CSR.** Fusionar las dos orientaciones en un array
  sería copiar el grafo entero para no ganar nada: la vecindad de un vértice es la concatenación de
  su tramo en cada fichero. Una contracción construye un lado simétrico y por tanto una lista de uno.

**Y dos cosas que el ADR estimó mal.** El diff son **+376/−40 líneas en un fichero**, de las que 79
son tres tests nuevos, contra las ≈ +45 previstas: la estimación contaba el código y no la prosa que
este repositorio le exige a cada invariante. Y el reloj de `community_hierarchy` sube de 135 a 168 s
— con la máquina cargada por otros agentes, y con el mismo binario anterior habiendo medido 151 s en
otra pasada, así que el número no es limpio; pero recorrer dos arrays por vértice en vez de uno tiene
un coste de localidad que es estructural y no ruido, y queda anotado como lo que hay que medir en
seco antes de darlo por gratis.

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
| `apply_resource_limits` (DuckDB) borrado | −40 · **refutada, ver abajo** |
| test: corpus pequeño, límite ridículo, exigir derrame | +60 |

**Neto ≈ +100 LOC**, y convierte un OOM en lentitud o en un error legible.

#### Ejecutada a medias el 2026-08-05, y es la etapa que pagaba

Se implementó sólo la primera fila —`RuntimeEnvBuilder` con `FairSpillPool` envuelto en
`TrackConsumersPool`, tras `FOSSIL_DF_MEM_GIB`— y bastó para responder la pregunta que ADR-0042 §5
dejó abierta: **la memoria de W0b no es el corpus.**

`arrow_gib` cuenta lo que el tipo de la frontera retiene de verdad: cada lote de vértices y las dos
orientaciones de cada tabla de aristas. A diez millones son **1,64 GiB**, con **15,68 GiB
residentes**. El grafo entero es la décima parte de lo que el proceso tiene cuando se lo entrega al
escritor.

Dos lecturas más lo confirman, y ninguna es un argumento: soltar el `SessionContext` antes de
codificar libera **0,00 GiB** —las `MemTable` del ejecutor son los mismos buffers Arrow, no una
segunda copia— y la misma fase midió **+11,35 GiB** en una ejecución y **+7,67 GiB** en la
siguiente. Dato vivo no varía en 3,7 GiB.

Lo que sí es: memoria de operador que nadie acota. El primer intento con 4 GiB murió, y el pool con
consumidores nombró al culpable sin necesidad de hipótesis:

    HashJoinInput[8]#125(can spill: false) consumed 387,7 MB, peak 387,7 MB,
    HashJoinInput[9]#126(can spill: false) consumed 291,9 MB, peak 291,9 MB,
    …cinco así, en una máquina de 10 núcleos: una reserva por partición

El lado de los vértices ya había demostrado que el presupuesto funciona —el orden/dedup sobre diez
millones de IRIs pasó de 7,94 a 3,02 GiB derramando, a cambio de diez segundos—. El lado de las
aristas junta cada extremo contra la tabla de vértices, y el lado de construcción de un hash join
declara `can spill: false`: cuando se le dice que no puede asignar, no tiene nada que devolver y
muere. Un sort-merge join sí derrama. De ahí que el presupuesto lleve `prefer_hash_join` apagado:
**un presupuesto sólo significa algo si los operadores pueden honrarlo.**

| a diez millones | sin techo | 4 GiB + sort-merge |
|---|---|---|
| RSS al terminar `execute_graph` | 15,68 GiB | **5,56 GiB** |
| RSS al empezar `enrich_layout` | 16,51 GiB | **6,82 GiB** |
| pico del proceso | ~21 GiB | **9,87 GiB** |
| reloj | 256,3 s | 290,9 s |

**Diecisiete gigabytes a diez, por un 13% de reloj, y el corpus sale idéntico byte a byte** (87
ficheros, todos los md5 iguales). Lo que queda arriba es Louvain (+2,22 GiB) y la maquetación, que es
donde ADR-0042 predijo el coste antes de que nada de esto estuviera medido. De ese +2,22 GiB, la
etapa 1 se llevó luego casi todo: no era Louvain, era el CSR que Louvain construía al entrar.

#### Terminada el mismo día: la bandera, los dos motores y el test

**El presupuesto es una entrada del `run`.** `fossil run --memory-gib <GIB>` (fracciones válidas, no
positivo se rechaza en el parseo) baja como `Option<u64>` de bytes por `fossil_engine::run` hasta
`fossil_df::run_to_dir`. `FOSSIL_DF_MEM_GIB` **no existe**: era andamio de una medición, y un número
del que depende que un trabajo sobreviva se declara en la orden que lo lanzó. Sin bandera, el
comportamiento de hoy — sin techo.

**Y aquí la tabla de arriba se equivocaba.** Decía borrar `apply_resource_limits` «que duplica».
Leído el código, no duplicaba nada: la maquetación **sigue ejecutándose en DuckDB**, así que borrar su
`memory_limit` no quita una duplicación, quita el único límite del segundo motor. Lo duplicado era la
*forma de declararlo* — tres variables de entorno (`FOSSIL_DUCKDB_MEMORY_LIMIT`, `_THREADS`,
`_TEMP_DIR`) contra una bandera. Así que la función no se borra: pasa a ser
`apply_memory_budget(conn, bytes)`, recibe **el mismo número** que el pool de DataFusion y fija
`memory_limit` más un `temp_directory` (un límite sin sitio donde derramar da error donde podía dar
lentitud). Las tres variables sí se borran. **Un presupuesto declarado, dos motores, ningún sitio
donde declararlo dos veces.** Lo que también se borra es la llamada de la pre-introspección (un
`DESCRIBE` de cabeceras no es donde se gasta memoria) y la de `fossil-mcp`, que no tiene presupuesto
que pasar.

**El test que exige derrame** (`crates/fossil-df/tests/spill.rs`, ~30 s): 5k personas y 400k pedidos,
dos pasadas sobre el mismo corpus. Sin presupuesto no toca disco; con él, DataFusion abre su
directorio de derrame y lo dice por `log` —única evidencia que sobrevive a la ejecución: los ficheros
se borran, `used_disk_space` vuelve a cero y las métricas del plan no salen de `execute_graph`—, y
los dos árboles escritos son **idénticos byte a byte**. Lo que no prueba: que diez millones quepan en
4 GiB (aquí no se mide RSS), ni que un plan libre de elegir hash join sobreviva (no lo hace: por eso
`bounded_context` apaga `prefer_hash_join`, y el test sólo ve el plan que eso deja).

**Y una medición que el test obligó a hacer: un presupuesto demasiado pequeño no derrama, muere, y el
suelo lo pone la máquina.** Cada partición de `sort` reserva `sort_spill_reservation_bytes` (10 MB) y
la declara `can spill: false`, así que el pool mínimo crece con `target_partitions`. En diez núcleos:
384 MiB muere con cinco `ExternalSorterMerge` reteniendo 10 MB cada uno, 480 MiB corre y derrama, y
sigue derramando a 2 GiB. Por eso el test pide 64 MiB **por núcleo** en vez de una cifra redonda. Los
4 GiB de la tabla de diez millones están holgadamente por encima de ese suelo; un presupuesto de 100
MB no es viable a ningún tamaño, y eso es parte del contrato, no un detalle de la implementación.

#### Y lo que esto le hace a la tesis del documento

La causa que da la §«La causa, que es de tipos» —seis representaciones, cada frontera una copia— se
midió aquí y **no es la que paga a esta altura**: la copia retenida es 1,64 GiB de 17. La etapa 1
sigue en pie porque su medición es otra y más abajo: el `Vec<(u32,u32)>` de 568 MB y el `cursor` de
80 MB son reales, y su justificación es el acceso aleatorio, no el pico total.

Dicho como corresponde: el tipo de la frontera explica lo que cuesta *cruzarla*; lo que hacía que el
proceso pidiera diecisiete gigabytes era **no haber declarado nunca un presupuesto**.

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
| 1 · CSR desde el artefacto ✔ | +336 *(est. +45)* | **−1,71 GiB medidos** | — |
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
