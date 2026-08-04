# ADR 0042: La cámara se direcciona, no se consulta

**Date:** 2026-08-04
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** ADR-0041 (la pirámide de contracción y el álgebra) y ADR-0039 (superficie/transporte), a los que
este revisa en tres puntos concretos; `kanzo-ui/BENCHMARKS.md`, de donde sale cada número de aquí.

## Contexto

El ADR-0041 fijó dos números que tenían que moverse. **El primero se movió y el segundo no**, y
medir por qué es lo que produjo este documento.

La retención de aristas en una ventana pasó de **0,26 % a 56,99 %** a un millón y a **63,65 %** a
cinco, al particionar por `community_hierarchy` en vez de por componentes conexas. Ese está cerrado.

El pan a cinco millones tenía que caer a la banda de 40 ms. Está en **256 ms** y, con la ventana ya
corregida a zoom constante, **sigue creciendo con N**: 25× el corpus cuesta 7× el pan. Sublineal, no
plano.

Buscando la causa se descartaron cinco palancas, todas plausibles y todas medidas falsas: que el
cuello fueran los enlaces; que las funciones de ventana desperdiciaran trabajo; que las consultas
concurrentes valieran un `max` en vez de un `sum`; que Hilbert mejorara la localidad sobre Morton; y
que hicieran falta las tablas de offset de GraphAr. Además el propio arnés medía mal de tres formas
distintas — una banda de altura completa, una apertura sobre todo el corpus y una ventana
proporcional al espacio — y cada arreglo invalidaba la conclusión anterior.

Lo único que no fue una corazonada fue preguntar **qué es una ventana dentro del fichero**:

- 20.000 vértices a cinco millones son **179 tramos contiguos de `dense_id` que cubren exactamente
  20.007 ids** — el 0,4 % del corpus, **sobrelectura cero**. La maquetación es grumosa, una comunidad
  es un disco compacto y una ventana contiene comunidades **enteras**; cada comunidad es un tramo
  Morton contiguo.
- Esos tramos contienen **130.516 de 34.974.279 aristas — 268×** — y devuelven las mismas 120.103
  aristas visibles. Es un superconjunto exacto, no una aproximación.
- **Y ninguna forma de pedirlo en SQL lo consigue.** Join normal 5 ms; range join contra los tramos
  237 ms; 179 predicados `BETWEEN … OR …` 189 ms y 2,3 s de CPU. Los dos intentos son más lentos que
  no podar, porque `DuckDB` evalúa los rangos por fila en vez de saltarse row groups.

**La poda no se puede expresar como predicado. Sólo se puede expresar como qué se lee.**

## Decisión

### 1. La cámara se direcciona; el álgebra se consulta

Dos APIs y **un solo camino de dibujado**.

La cámara calcula nivel y teselas y pide bytes. No hay bbox en vuelo, no hay SQL, no hay `DuckDB` ni
Mosaic en ese camino. El ADR-0041 §3 quería `viewport` como «un plan con nombre»; con
direccionamiento **no es un plan: no es nada**.

Y no es sólo economía. **El LOD no es filtrar, es leer otra relación**: una tesela de nivel 3
contiene supernodos que no existen en el nivel 0. Un `where` selecciona filas de una tabla; el zoom
cambia de tabla. El `if` que hoy alterna detalle y agregado dentro de `viewport` era selección de
relación disfrazada de predicado.

El álgebra queda para preguntas —filtros, agregados, esquema, caminos—, devuelve Arrow y **no
dibuja**. Si un filtro debe cambiar la imagen, devuelve **ids** y el lienzo los aplica como máscara
sobre las teselas residentes. Un mecanismo, no un segundo renderizador.

**Regla para no acabar con cinco caminos:** uno nuevo sólo entra si sirve un caso que el único camino
demostrablemente no puede, y esa demostración es una medición.

### 2. Parquet a secas. Fuera GraphAr

GraphAr **dio vocabulario y dejó de dar contrato**. Lo bueno —CSR y CSC, ids densos, alinear aristas
al chunk del origen— ya está pensado y se queda como invariante interna. Lo que costó está medido: su
manifiesto no lleva recuento de vértices ni dice cómo descubrir chunks, así que hubo que inventarse
la derivabilidad; y dos preguntas que se declararon bloqueantes existían **sólo** por conformidad.

Como formato de intercambio tampoco paga: quien reciba el corpus tendría que aprenderse una
especificación en incubación. `vertices.parquet` y `edges.parquet` con nombres que se explican solos
son **más** interoperables, no menos.

Parquet además ya es lo que a Zarr le llegó como extensión: un fichero con row groups y un footer que
es su índice. Una tesela es un row group; el footer dice en qué bytes está. Y eso no contradice el
hallazgo de que los row groups no podan: **direccionar no es filtrar**, y quien lee el footer y pide
un rango de bytes es el cliente, no el planificador.

### 3. Los niveles de la jerarquía son los niveles de LOD

`community_hierarchy` ya devuelve **todos** los niveles y hoy se usan dos: el 0 para colocar y uno
para `cluster_id`. Los demás se descartan. **Son la pirámide.** El ADR-0041 §1 lo dijo —«la pirámide
es la jerarquía de clusters»— y se implementó a medias.

Una tesela es una comunidad, con **presupuesto de puntos constante y extensión variable**. No es una
rejilla ni un rango de `dense_id` de tamaño fijo: eso último fue lo que se construyó y es una
aproximación gruesa de esto.

Las columnas se disponen para subir a GPU sin transformar —`x` e `y` como arrays separados— lo que
elimina por construcción el interleave que hoy hace `arrays()` en el hilo principal. Una arista vive
en la tesela más profunda que contiene ambos extremos.

### 4. Las posiciones se toman si vienen y se calculan si no

`enrich_layout` hoy calcula `x`/`y` incondicionalmente. Con posiciones dadas —geográficas, o
ajustadas a mano— la maquetación es opcional y ésa pasa a ser la rama normal.

**Las posiciones movidas a mano son una capa encima, nunca una reescritura**: las teselas son
precomputadas y mutar implicaría reconstruirlas.

### 5. El núcleo puro sale de `fossil-runtime`

`community_hierarchy` y `cluster_layout` son puros —sin I/O, sin `DuckDB`, sin RNG— y viven presos en
el crate que posee `DuckDB`, así que no son wasm-limpios. Salen a su propio crate (~420 líneas con
sus tests). El README de `fossil-graph` justifica hoy esa ubicación por necesitar el conjunto de
aristas resuelto: eso es cierto de **la pasada que aplica**, no del algoritmo.

### 6. cosmos.gl no se toca

Es la única capa que **ninguna medición ha implicado**: 0,5–3 ms de redibujado a cualquier tamaño, sin
seguir a N. Y su simulación en GPU es lo que hace posible arrastrar un nodo y que los vecinos se
reacomoden.

## Consecuencias

**Balance de líneas.** Fuera: GraphAr entero; el verbo `viewport` (~500 entre `exec.rs` y
`operations/`); `bounded.ts`, `memory-source.ts`, `duck-source.ts` y `adaptive.ts` (767); el
renumerado Morton con su emisión de chunks, el remapeo de adyacencias y su test de integración (~600,
escrito el 2026-08-03); `flatten_to_budget`, `order_by_hierarchy` y `weakly_connected_components`
(~225). Dentro: emisión de teselas, un almacén de teselas y un índice (~430). **Neto ≈ −1.400.**

**Los verbos pasan de 17 a 6** — `read`, `expand{into|all}`, `path`, `aggregate`, `schema`,
`execute_sql`. Tres de los cuatro no implementados desaparecen sin implementarse: `summarize_cluster`
y `answer_with_communities` son un `read` sobre un nivel, y `set_selection` siempre fue del cliente.
**Trampa anotada:** el ADR-0041 justifica `expand{into}` citando la máscara que empujan Kùzu y Neo4j
dentro del escaneo; hoy está medido que **`DuckDB` no acepta esa clase de poda**. El verbo está bien
como forma; su motivación de rendimiento no está disponible en nuestro motor.

**`memory-source` desaparece** porque «el grafo entero en memoria» es una tesela residente, y «tesela
desde HTTP» y «tesela desde memoria» son la misma implementación con otro `fetch`. `BoundedSource`
pierde la ceremonia —modos, límites, umbrales, `matched`— y conserva la costura: el lienzo no sabe de
dónde vienen los buffers.

**El riesgo que más bugs puede dar, y va primero.** `use-graph-selection.ts` y `use-graph-overlays.ts`
trabajan sobre índices locales de la porción actual. Con un conjunto residente que entra y sale, ese
índice deja de ser estable aunque las identidades sí lo sean. **Pasar selección y overlays a identidad
antes de tocar teselas**, con el visor actual, porque es verificable hoy y aísla el riesgo.

**Un crossfilter arbitrario sólo filtra lo residente.** Mapbox tiene el mismo límite y tampoco lo
resuelve; por eso `read(where:)` sigue siendo el camino general.

## Verificación

Tres demos, cada una atacando un hueco distinto, y las tres son el banco de pruebas nuevo — porque el
actual mide un corpus sintético, de un solo tipo, que cabe en RAM.

**Grafo de conocimiento, primero**, porque es el que puede invalidar el diseño antes de construir
encima. Todo lo medido hasta hoy es **un tipo de vértice y un tipo de arista**; `place_after` nunca se
ha ejercitado a escala y las aristas entre tipos distintos están excluidas de la maquetación por
construcción. La jerarquía nunca ha visto un grafo multi-tipo.

**Mapa con lat/lon, después.** Fuerza que las posiciones dadas sean la rama normal, y comprueba algo
fino: que el árbol de teselas salga igual sobre posiciones dadas que sobre calculadas. Si no, la
jerarquía está acoplada a nuestra colocación.

**Larger-than-RAM, al final**, porque es la más cara. Es **la afirmación central de la arquitectura y
no se ha probado nunca**: el corpus mayor son 5M ≈ 100 MB. La métrica que decide no es el pan sino el
**pico de heap**, que debe quedarse plano. Y el riesgo no está donde miramos: Louvain corre en memoria
sobre el grafo entero, y el generador ya reventó contra el tope de 512 MB de string de V8 a 35M
aristas. **Que fossil no pueda escribir un corpus larger-than-RAM sería un hallazgo tan importante
como cualquiera de lectura.**

Se reutiliza el arnés existente —retención, chunks tocados, pan a zoom constante, sonda de solape de
conexiones— y falta una métrica: **bytes descargados por paneo**, que es lo único que justifica las
teselas frente a un fichero único y sigue sin medirse.
