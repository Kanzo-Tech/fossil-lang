# ADR 0041: Una pirámide de contracción, y un álgebra componible que la lee

**Date:** 2026-08-03
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** ADR-0039 (separación superficie/transporte, y el techo de "~1M vertices" que nombra); ADR-0001 en `kanzo-ui` (el lienzo pregunta, no sostiene); `kanzo-ui/BENCHMARKS.md` capa 4 y `kanzo-ui/packages/graph/FOSSIL-VIEWPORT.md` (todas las cifras).

## Contexto

ADR-0039 puso el conocimiento de GraphAr en un sitio y separó la superficie de sus transportes.
Acertó, y nombró lo que había que batir: los caminos previos *"topped out at ~1M vertices"*. Medido
ahora **a través de la superficie que aquel ADR creó, el techo sigue siendo ~1M** — y lo útil es que
la razón ha cambiado.

Corpus escrito por `fossil run`, leído con DuckDB-WASM ejecutando exactamente las dos sentencias que
emite `viewport`:

| Vértices | Aristas | Primer pintado | Pan | Actualizaciones/s |
|---|---|---|---|---|
| 200k | 1,37M | 97 ms | 41 ms | 24,2 |
| 1M | 6,90M | 240 ms | 93 ms | 10,7 |
| 5M | 34,97M | **1.006 ms** | **331 ms** | **3,0** |

Lo que queda plano es real y es media arquitectura: `count(*)` a 7–9 ms porque son metadatos de
Parquet, la transferencia a 23 ms porque la respuesta está topada a 20.000 marcas, el renderizador
por encima de mil fps a cualquier tamaño. **La ventana acota todo lo que se dibuja y se transfiere.**
Lo que no acota es lo que se **escanea**: el predicado bbox y el join de aristas son O(N). Desglose a
1M en caliente: vértices 25–29 ms, aristas 33–34 ms, `count(*)` 9–10 ms, y Arrow IPC ~10 ms.

*Renderizado acotado, consulta no acotada.*

Dos intentos de fingir un índice con estadísticas de row group **empeoraron las dos veces**
(1M: 219→244 ms; 5M: 974→1.072 ms), el segundo en el tamaño que era la excusa del primero.

Y hay un problema anterior a la velocidad: **una ventana conserva 375 de 27.244 aristas incidentes,
el 1,4 %**, apenas cuatro veces el azar. WCC sobre un grafo conexo da una componente y W3.1 la coloca
como una espiral filotáctica de radio 12·√n. **Acelerar el acceso a eso es acelerar el acceso a
ruido.**

## Decisión

### 1. La pirámide es la jerarquía de clusters, no una rejilla espacial

Leiden no produce una partición sino una *jerarquía*: nodos → comunidades → comunidades de
comunidades. Eso ya es una pirámide, derivada de la topología. `contract` en cada nivel es el grafo
cociente, y **deja de ser un verbo de consulta para pasar a ser cómo se construye cada nivel**.

**Dos ejes**: el nivel es profundidad de clustering (topología); el chunk es la subdivisión dentro
del nivel (geometría). Cada nivel se maqueta por separado — layout multinivel clásico, del que
`cluster_layout` ya hace media pieza.

### 2. GraphAr ya especifica el chunking, incluidas las aristas. Hay que implementarlo, no extenderlo

El manifiesto que ya escribimos declara `chunk_size`, `prefix: vertex/<Type>/`,
`src_chunk_size`/`dst_chunk_size` alineados al chunk de vértices, y **dos** `adj_lists`
(`aligned_by: src` y `aligned_by: dst` — CSR y CSC). Es decir, GraphAr particiona las aristas **por
el chunk del vértice origen** y guarda ambas direcciones, así que una arista es descubrible desde
cualquiera de sus extremos.

**El escritor no emite nada de eso**: salen `vertex/<Type>.parquet`, `by_source.parquet` y
`by_target.parquet` como ficheros únicos. `fossil-sinks/src/manifest.rs:16` lo dice — la emisión por
chunks *"lands in plan 05-08"*. Estaba planificada y aplazada.

**Un chunk de Parquet ya es una tesela con su índice dentro**: el min/max de `x`/`y` de su footer es
su bounding box, así que el índice espacial no hay que construirlo.

**Pero con el escritor de hoy el chunking de GraphAr NO sería espacial.** Medido sobre el corpus de
cinco millones partido en 41 chunks, para una ventana de 22.216 filas:

| Chunking por | Chunks tocados | Filas leídas |
|---|---|---|
| `dense_id` (lo que GraphAr define) | **41 de 41** | 5.000.000 |
| Orden físico de fila (Morton) | **6 de 41** | 737.280 |

`finalize_vertex` (`fossil-df`) ordena por `subject` y numera; `enrich_layout` (`fossil-runtime`)
aplica el orden Morton **después**, como reescritura de filas. El orden *físico* del fichero es
Morton, pero los *valores* de `dense_id` van en orden de IRI — y GraphAr define el chunk *i* como los
`dense_id` de `[i·chunk_size, (i+1)·chunk_size)`. Sobre orden de IRI, cada chunk abarca todo el
espacio y no poda nada.

**Para que un chunk sea una tesela, `dense_id` debe asignarse en orden Morton.** Y eso cuesta más de
lo que parece: **las tablas de aristas referencian `dense_id`**, así que renumerar los vértices
obliga a remapear ambos extremos de todas las aristas — 35 M a cinco millones de nodos. Es
precisamente por eso que `enrich_layout` hoy toca sólo `x`/`y`/`cluster_id` y deja la numeración en
paz. La dependencia de orden real es: posiciones → Morton → `dense_id` → aristas, y hoy el layout
corre después de la fase de aristas.

Un borrador anterior de este ADR inventó una política de «arista al ancestro común más grueso». Era
innecesaria: GraphAr la resuelve guardando ambas direcciones.

**La ganancia no es de escaneo, es de caché.** Un bbox es continuo y cada paneo es un fallo; un chunk
es discreto, direccionable por URL y cacheable por navegador y CDN, así que un paneo reutiliza la
mayoría de las teselas del frame anterior con cero bytes. Eso también es lo que distingue esto del
experimento fallido de row groups: 611 row groups en un fichero de 97 MB comparten un footer y un
recurso HTTP; 611 chunks son 611 URLs.

**Lo que GraphAr no tiene es LOD ni pirámide** — es un formato para *un* grafo. Los niveles se añaden
**como convención (un grafo GraphAr por nivel dentro de un grupo)**, no extendiendo el formato.

### 3. El álgebra: CRUD más lo que CRUD no sabe decir

- **`read`** sobre `vertices` | `edges` con predicado, proyección, orden, límite. Absorbe
  `get_vertex`, `search_by_label`, `top_k` y las facetas del crossfilter. Un bbox es un predicado; el
  chunk es un *access path* que el planificador elige y `explain` enseña.
- **`expand{mode}`** unifica el subgrafo inducido y la expansión k-hop, siguiendo a Neo4j
  (`Expand(Into)` vs `Expand(All)`). **No es un self-join**: Kùzu (`SEMI_MASKER`) y Neo4j empujan una
  máscara de pertenencia dentro del escaneo. Los 33–34 ms medidos son de la formulación, no de la
  operación.
- **`path`** aparte. **`contract`** sale del álgebra de consulta y pasa al compilador (ver §1).
- **`aggregate`** absorbe `histogram` y `top_k`; **`schema`** absorbe los cuatro `list_*`/`describe_*`
  y mata el N+1 actual (`list_vertex_types` hace un `COUNT(*)` por tipo en bucle); **`execute_sql`**
  se queda como escape con rol.

**Cuatro de los 17 verbos actuales no están implementados** (`search_by_label`, `summarize_cluster`,
`answer_with_communities`, `set_selection`): parte del diseño es decidir cuáles merecían existir.

**La composición vive en la consulta, nunca en el RPC.** Partir `viewport` en llamadas separadas sería
peor que el compuesto: `expand{into}` necesita 20.000 ids y una segunda llamada compra un viaje de
ida y vuelta para ahorrar un join. Un plan, una ejecución. `viewport` pasa a ser un **plan con
nombre** que un host usa, inspecciona o sustituye; `zoom` y `lod_threshold` salen del motor, porque
el host tiene la cámara.

### 4. Perezoso, streaming y columnar — con el alcance dicho de verdad

- **Plan como valor**: construir → `explain`/costear → ejecutar al tirar.
- **Dos terminales, estilo Polars**: `collect()` devuelve en memoria, `sink()` nunca materializa. Eso
  hace que *larger-than-RAM* sea una propiedad de qué terminal llamas, no una afirmación de
  diapositiva.
- **Describir antes de traer**, estilo Arrow Flight: esquema y filas estimadas antes de mover bytes.
  Resuelve por forma de protocolo el tercer viaje que hoy calcula `n`.
- **Resultado valor-grafo con transición tipada** (como GFQL de Graphistry): un `RecordBatchStream`
  es de esquema único y un grafo son dos relaciones.
- **El streaming es real para el escaneo y la contrapresión, no para todo.** `expand{into}` es
  *pipeline breaker* (`EmissionType::Final` en vocabulario DataFusion): necesita el conjunto entero
  antes de emitir una arista.
- **La pereza no empuja el límite** hasta el escaneo: el pushdown para en los breakers y el límite ya
  está en el `read`. Las ganancias reales son *projection pushdown*, una ejecución en vez de tres
  viajes, e inspección del plan.
- **Columnar de extremo a extremo**: `query_json` deja de ser la primitiva del ejecutor. La forma
  fila-JSON sobrevive sólo donde un transporte la necesita de verdad (MCP), producida en el borde.

### 5. Lo que NO se hace

**No unificar motores en DataFusion.** El SQL son ~75 ms de un slice de 217 y Arrow IPC ~10 ms: el
margen no paga reescribir ~900 líneas de construcción de SQL como construcción de planes, ni ~7 MB gz
de DataFusion en el camino de consulta, ni perder el httpfs de DuckDB-WASM que es lo que hace
funcionar los *range requests*. `fossil-df` sigue siendo DataFusion en escritura y `fossil-graph`
DuckDB en lectura.

## Consecuencias

**Radio de impacto, medido y no supuesto** — un borrador anterior se equivocó aquí: **`fossil-cli` no
usa `fossil-graph`** (sólo lo enlaza transitivamente vía `fossil-engine → fossil-runtime`) y
**`fossil-http` no existe**. Lo que sí rompe: `fossil-graph`; `fossil-runtime/src/graph_exec.rs`
(107 líneas) y `tests/graph_verbs.rs`; `fossil-graph-wasm/src/lib.rs` (~110); `fossil-mcp` (sólo el
tipo de retorno, su herramienta es genérica); `packages/graph/src/{client,generated}.ts`; y **35
snapshots insta**, que fallan por diseño y son el detector de regresión.

**Un crossfilter arbitrario no se responde desde teselas precalculadas** — se compilan para el grafo
sin filtrar. Mapbox tiene el mismo problema y tampoco lo resuelve. Por eso `read(where:)` no
desaparece: es el camino general, y la tesela es el camino rápido del caso espacial sin filtrar.

**Renumerar por Morton es el trabajo escondido.** «Emitir chunks» suena a terminar algo declarado;
en realidad arrastra reordenar `dense_id` y remapear los extremos de todas las aristas. Presupuestarlo
como una tarde es el error a evitar.

Leído el código (2026-08-03), el alcance es concreto y **no** es el que este ADR suponía:

- **No hace falta mover el layout por delante de la fase de aristas.** `enrich_layout` ya corre al
  final, ya tiene la conexión DuckDB y ya reescribe el Parquet de vértices en orden Morton. Renumerar
  ahí y remapear los ficheros de aristas *que ya existen* es estrictamente menos invasivo que
  reordenar las fases: el remapeo es un join contra una tabla de correspondencia, no un cambio de
  arquitectura.
- **`VertexLayoutTarget` no ve las aristas que tendría que reescribir.** Hoy lleva sólo
  `by_source.parquet` de las aristas del mismo tipo (`fossil-engine/src/lib.rs:412`). Renumerar un
  tipo obliga a reescribir *todo* fichero que lo referencie en cualquiera de sus dos extremos:
  también `by_target.parquet`, también las aristas entre tipos distintos. Eso es un cambio de firma y
  de llamante, y con dos tipos renumerados cada fichero de aristas necesita los dos remapeos, cada
  extremo contra el mapa de su propio tipo.
- **La trampa que no estaba listada: `adj_lists` se declara `ordered: true`.** El remapeo invalida
  ese orden — un CSR ordenado por `src_dense` deja de estarlo en cuanto los `src_dense` cambian de
  valor. Los dos ficheros no se remapean: se remapean **y se reordenan**. Un lector GraphAr que se
  fíe del manifiesto leería basura, y no habría error que lo dijese.
**Renumerado: hecho y medido (`fossil-runtime` fc2a892).** `enrich_layout` asigna `dense_id` en
orden Morton y remapea todas las listas de adyacencia. Cinco millones, 41 chunks, ventana de 3.500
nodos:

| | por `dense_id` (lo que lee un lector) | por orden físico |
|---|---|---|
| antes | 40,8 de 41 | 2,0 de 41 |
| después | **2,0 de 41** | 2,0 de 41 |

**Que las dos columnas coincidan es el resultado.** La retención es el control y no se movió —56,99 %
y 63,65 %— porque renumerar cambia qué entero lleva un vértice, no dónde está. Ese control sólo
funcionó tras arreglar el arnés: elegía los centros de ventana por `dense_id`, que es justo lo que
se está midiendo, y la primera comparación muestreó ventanas distintas en los dos corpus. **Nunca
sembrar una medición con un valor que el cambio bajo prueba puede mover.**

Confirmadas las tres trampas del listado anterior, y todas con test porque ninguna falla sola.

**Chunks de vértices: emitidos (`fossil-runtime` c678e63).** `vertex/<Type>/chunk{k}.parquet`, con
el `chunk_size: 1024` que el manifiesto declaraba desde el principio — medido, no supuesto. La
sobrelectura es chunks tocados × tamaño, y los chunks tocados apenas crecen al encogerlos porque la
localidad Morton hace que una ventana cubra un *área* casi constante:

| `chunk_size` | chunks tocados | sobrelectura |
|---|---|---|
| 1.024 | 9,8 de 4.883 | **2,9×** |
| 8.192 | 3,4 de 611 | 8,0× |
| 122.880 | 2,0 de 41 | 70× |

**Una ventana descarga 10 de 4.883 chunks: el 0,2 % del corpus.** La retención vuelve a ser el
control y no se movió. Escribir 4.883 chunks cuesta segundos (200 en 0,18 s, 20 kB cada uno);
`PARTITION_BY` sería una sentencia pero emite `chunk=0/data_0.parquet` en vez del nombre de ADR-0016.

**Y la afirmación de caché, medida sobre un paneo** (`corpus/measure-pan.mjs`), porque chunks
tocados sale igual con ficheros que con rangos y no puede ver lo que compró emitirlos por separado.
Ocho pasos de arrastre de un cuarto del ancho de la ventana, cinco millones:

| `chunk_size` | aciertos | chunks pedidos | **filas en todo el paneo** |
|---|---|---|---|
| 1.024 | 85 % | 21 de 79 tocados | **21.504** |
| 8.192 | 97 % | 6 | 49.152 |
| 32.768 | 100 % | 4 | 131.072 |
| 122.880 | 100 % | 3 | 368.640 |

**La tasa de aciertos es una trampa.** Mejora con chunks más grandes por la razón que la invalida: un
chunk que contenga el paneo entero se pide una vez y no vuelve a fallar, así que puntúa perfecto por
haberlo descargado ya todo. La columna comparable es la carga, y ahí 1.024 gana por diecisiete veces.

Dos cosas que no conviene redescubrir. **Un glob es la forma equivocada de leerlos**: expandir
`*.parquet` es listar un directorio, y un origen HTTP plano no tiene listado — el httpfs de DuckDB
*sí* puede contra S3, así que el error funciona con `file://`, funciona con un bucket y falla en el
navegador. El lector deriva la lista del recuento de vértices y `chunk_size`. Y **`--row-group`
desaparece**: un chunk de 1.024 filas *es* un row group.

Queda **el chunking de aristas**, y tiene una pregunta de diseño abierta que conviene resolver
antes de escribir nada. GraphAr las particiona por el chunk del vértice origen (`src_chunk_size`) y
luego trocea cada partición en chunks de `chunk_size` aristas. Los dos números están en `EdgeInfo`,
pero **cuántos chunks tiene la partición *i* no sale del manifiesto**: depende de cuántas aristas
salgan de esos 1.024 vértices, que es una distribución de grado y en un grafo hiperbólico es una ley
de potencias. Un lector de GraphAr resuelve eso listando el directorio — y ya sabemos, por el error
de los vértices, que **sobre HTTP plano no hay listado**. Así que hace falta o un índice emitido
junto a las aristas, o una convención que haga derivable el recuento. Elegir eso es el primer paso,
no la emisión.

Mientras tanto las aristas se renumeran y se reordenan pero se emiten enteras. La poda por bbox es un
escaneo de vértices, así que lo ya emitido es la mitad que poda; el join de aristas sigue siendo
O(N).

- **`chunk_size: 1024` no sobrevive a cinco millones como tamaño de escritura.** Son 4.883 chunks de
  vértices más los de aristas, y la convención de nombre (`<prefix>chunk{k}.parquet`, ADR-0016) no la
  produce `PARTITION_BY` de DuckDB, que emite `chunk=0/data_0.parquet`. Sale un `COPY` por chunk, es
  decir casi cinco mil sentencias. O el tamaño sube, o la emisión no es un bucle de `COPY`.

**No rompe el techo por sí solo lo columnar**, y no debe venderse así: quita el marshalling y la
materialización de golpe, no hace sublineal un escaneo O(N). Lo que rompe el techo es la pirámide.

**Existe una arquitectura alternativa que no se descarta.** Cosmograph y Graphistry sostienen el
grafo entero en GPU (~320 MB de buffers a 5M): carga alta y única, luego paneo gratis. El cruce está
en torno a una docena de movimientos de cámara. La ruta acotada gana en tiempo-hasta-la-primera-
imagen y en corpus que no caben; la residente gana en exploración sostenida. **La síntesis probable
es que la ruta acotada sea el camino de arranque en frío y la GPU se hidrate por detrás.**

## Verificación

`/view/showcases/graph-bench` en `kanzo-ui` ejecuta las mismas dos consultas que el verbo hasta cinco
millones y separa primer pintado, pan, actualizaciones/s y techo de redibujado. **Dos números tienen
que moverse: la retención de aristas en una ventana, que valida la jerarquía y va primero; y el pan
a cinco millones (331 ms), que debe caer a la banda de 40 ms y dejar de crecer con N.** Controles que
deben seguir planos: `count(*)` 7–9 ms, transferencia 23 ms, techo de redibujado en miles de fps.

### El primero, medido (2026-08-03)

`enrich_layout` particiona por `community_hierarchy` en vez de por componentes conexas
(`fossil-runtime` 9ee4770). Arnés: `corpus/measure-retention.mjs`, cinco ventanas de 3.500 nodos cada
una, definidas **por rango** — el cuadrado más pequeño centrado en un nodo que contiene exactamente
*k* — porque un rectángulo fijo atrapa recuentos muy distintos en dos maquetaciones y reportaría una
diferencia que es sobre todo el recuento.

| | conservadas / incidentes | retención | nulo | bola |
|---|---|---|---|---|
| 1M, WCC | 589 / 225.448 | **0,26 %** | 0,17 % | 4,55 % |
| 1M, comunidades | 78.094 / 137.024 | **56,99 %** | 0,21 % | 4,55 % |
| 5M, comunidades | 91.119 / 143.147 | **63,65 %** | 0,04 % | 1,71 % |

Llegar ahí requirió una separación que no estaba en este ADR. `cluster_id` y la
colocación responden a preguntas distintas y no pueden ser la misma partición: el modo
agregado responde un supernodo por clúster bajo un `LIMIT`, así que `cluster_id` tiene
que quedarse grueso, mientras que la colocación quiere lo contrario — comunidades lo
bastante pequeñas como para que quepan varias en una ventana. Con una sola partición
para ambos usos la maquetación se quedaba en 13,63 %, porque el presupuesto la empujaba
a la cima de la jerarquía, donde toda comunidad es raíz y ordenar hermanos no ordena
nada. Separadas, 56,99 %.

Tres correcciones salen de medirlo, y las tres afectan a lo que este ADR afirmaba:

- **El 1,4 % estaba puntuado contra un nulo del doble de lo que toca.** Para una ventana aleatoria
  `conservadas ≈ E·(k/N)²` frente a `incidentes ≈ 2E·k/N`, así que el azar es `(k/N)/2` y no `k/N`.
  La maquetación vieja no era «cuatro veces el azar» sino 1,5 veces: aún más plana de lo que decíamos.
- **La bola BFS es una referencia, no un techo.** Es lo que da la topología sola, sin maquetación de
  por medio, y la maquetación por comunidades la triplica — una bola gasta casi todo su presupuesto
  en una frontera cuyas aristas apuntan todas hacia fuera. El máximo alcanzable sigue sin acotarse.
- **`cluster_layout` tenía un defecto que la partición vieja escondía.** Un clúster de *n* se empaqueta
  en un disco de radio `12·√n`, que pasa el paso fijo de 100 unidades a los 70 vértices; una única
  componente gigante no tiene vecino con quien solaparse, y por eso nunca se vio. El paso se mide
  ahora desde el clúster más grande.

Lo que **no** cambia: el algoritmo es Louvain, no Leiden. Falta la pasada de refinamiento que
garantiza que una comunidad esté internamente conexa. Y la maquetación sigue siendo de un solo nivel
— la retícula ordena las comunidades por id, así que dos comunidades muy conectadas caen lejos por
casualidad. Ambas cosas son trabajo pendiente, no supuestos ya cobrados.
