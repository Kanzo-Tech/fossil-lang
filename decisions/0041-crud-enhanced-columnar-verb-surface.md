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
en realidad arrastra reordenar `dense_id` y remapear los extremos de todas las aristas, y mover el
paso de layout por delante de la fase de aristas. Presupuestarlo como una tarde es el error a evitar.

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
| 1M, comunidades | 27.366 / 200.839 | **13,63 %** | 0,16 % | 4,55 % |
| 5M, comunidades | 13.669 / 231.853 | **5,90 %** | 0,06 % | 1,71 % |

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
