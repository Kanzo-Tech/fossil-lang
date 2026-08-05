# ADR 0042: La cámara se direcciona, no se consulta

**Date:** 2026-08-04
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** ADR-0041 (la pirámide de contracción y el álgebra) y ADR-0039 (superficie/transporte), a los que
este revisa en tres puntos concretos; ADR-0045 §8 (ids planos), que es lo que hizo computable la
disputa de §3.2; `kanzo-ui/BENCHMARKS.md` y `kanzo-ui/.planning/GRAPH-ROADMAP.md`, de donde sale cada
número de aquí.

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
  Morton contiguo. *(La segunda frase es falsa y está medida como falsa en §3: los tramos los produce
  la curva, no las comunidades. El primer número — los 179 tramos y la sobrelectura cero — sobrevive
  intacto, y con mejor explicación.)*
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

### 3. Una tesela son 4.096 filas de `dense_id`; encima no hay árbol, hay una función

**Reescrito el 2026-08-05.** Este apartado eran dos afirmaciones y sólo una se sostiene.

| | veredicto |
|---|---|
| «los niveles de la jerarquía son los niveles de LOD» | **confirmado** |
| «una tesela es una comunidad» | **refutado** |

`community_hierarchy` ya devuelve **todos** los niveles y hoy se usan dos: el 0 para colocar y uno
para `cluster_id`. Los demás se descartan. **Son la pirámide de agregación.** El ADR-0041 §1 lo dijo
—«la pirámide es la jerarquía de clusters»— y se implementó a medias. Eso sigue en pie.

Lo que no sigue en pie es «una tesela es una comunidad, con presupuesto de puntos constante y
extensión variable». Descansaba en que la maquetación es grumosa, una comunidad es un disco compacto
y una ventana contiene comunidades **enteras**. Medido a cinco millones, ni lo uno ni lo otro:
`community` son ocho grupos de 625.000 vértices y cada grupo está roto en **5.461 tramos** de
`dense_id`; `cluster_id` son **15.310 grupos de mediana 1 vértice** y p90 de 3; y una ventana de
20.000 vértices son **~170 tramos de ~118 ids**, que no es ninguna de las dos cosas.

**Los tramos los produce la curva, no las comunidades:** un rectángulo sobre un orden Morton se parte
en O(√n) segmentos, y ése es el mecanismo entero. El hallazgo del que salió este ADR —una ventana son
unos cientos de tramos contiguos con sobrelectura cero— **no sólo sobrevive: es más fuerte que cuando
se escribió**, porque ahora se sabe que no depende de que la maquetación agrupe bien. Lo que no se
sostiene es la pirámide que se construyó encima.

**Son dos estructuras ortogonales, no una.** La jerarquía da los niveles de *agregación*; el orden
Morton da los rangos de *bytes*. Este apartado las confundió en una sola, y por eso al caer la mitad
del fetch parecía caer también la del LOD. Las comunidades siguen siendo fieles **como agregados**
—p90 de radio 20 sobre un lienzo de 5.289.638 de ancho, contra 72.722 de un bin contiguo de la curva
del mismo tamaño—; lo que no son es una unidad de lectura.

#### 3.1. La unidad son 4.096 filas, y dentro de la banda no se afina

Una tesela es un rango fijo de `dense_id` de **4.096 filas** — `[i·4096, (i+1)·4096)` —, vértices y
aristas indexados por el mismo rango. Eso es exactamente «un rango de `dense_id` de tamaño fijo», que
este apartado descartaba por burdo, y la medición dice que era la respuesta.

Medido el 2026-08-05 sirviendo el corpus por un origen HTTP y contando cada petición que responde
(`kanzo-ui/…/corpus/measure-requests.mjs`; tabla completa en `kanzo-ui/BENCHMARKS.md`). A cinco
millones, contra una carga ideal de 0,6–0,9 MB por ventana que es **plana en N**:

| T | peticiones | bytes | vs ideal |
|---|---|---|---|
| 1.024 | 178 | 1,11 MB | 1,73× |
| 4.096 | **78** | 1,48 MB | 2,31× |
| 8.192 | 47 | 1,89 MB | 2,95× |
| 32.768 | 25 | 4,61 MB | 7,20× |
| 122.880 (el chunk de hoy) | 35 | 11,73 MB | 18,3× |
| el corpus tal cual está | 208 | 12,12 MB | 18,9× |

Los bytes tocan fondo en 1.024–2.048 y las peticiones caen monótonas, así que **las dos curvas no
tienen óptimo común** y lo que elige es `λ·β`, los bytes que mueve un enlace en la latencia de una
petición: en serie, 32.768; con seis en vuelo, 8.192; **multiplexado del todo, 4.096**. Un lector de
teselas computa todas las URL antes de emitir la primera, que es el contenido entero de *se
direcciona, no se consulta*, así que la unidad es 4.096.

**Y la banda de bytes es plana de 1.024 a 8.192, así que el emisor no se afina dentro de ella.** Un
cambio ahí no es una mejora, es ruido con un `git blame` encima.

**`chunk_size` 122.880 se retira.** No es una elección conservadora: está **dominado por 32.768 en
las dos curvas a la vez** —más peticiones *y* cuatro veces los bytes—, porque una tesela de 122.880
filas de aristas cruza varios row groups y cuesta 9,4 peticiones. Además no es potencia de dos, lo
que obliga a dividir donde un desplazamiento basta (ADR-0045 §8). `DEFAULT_CHUNK_SIZE`
(`fossil-sinks/src/manifest.rs:164`) pasa a 4.096.

#### 3.2. Una arista vive en la tesela de su origen. CSR, no LCA

Este apartado decía «una arista vive en la tesela más profunda que contiene ambos extremos» (LCA)
mientras §2 conservaba «alinear aristas al chunk del origen» (CSR) como invariante interna. **Son
colocaciones distintas y el ADR afirmaba las dos.** Se queda CSR.

Dejó de ser discutible en cuanto la tesela fue un rango fijo: con 4.096 filas la tesela de un vértice
es `dense_id >> 12`, el antepasado común más bajo de dos vértices es el prefijo común de sus ids —un
`XOR` y un conteo de ceros a la izquierda—, y el efecto que se temía **se puede medir sobre los
corpus que ya existen, antes de escribir un emisor** (ADR-0045 §8). Medido con
`spikes/lca-vs-csr/`, sobre los cuatro corpus del banco y con la ventana calcada del arnés: cuadrado
por rango que contiene 20.000 vértices, seis pasos de medio ancho.

**Dónde caen las aristas, por nivel.** Nivel 0 es «los dos extremos en la misma hoja»; la raíz es la
tesela que comparten todos los ids:

| N | hojas | profundidad | en nivel 0 | en la raíz |
|---|---|---|---|---|
| 200.000 | 49 | 6 | 88,51 % | 24.942 |
| 1.000.000 | 245 | 8 | 80,72 % | 98.429 |
| 5.000.000 | 1.221 | 11 | 74,42 % | 292.410 |
| 10.000.000 | 2.442 | 12 | 73,33 % | 621.102 |

**La raíz crece con N y la fracción que cabe en una hoja encoge**: 24.942 → 621.102 aristas mientras
el corpus se multiplica por cincuenta. Y la raíz **la paga toda ventana**, se dibuje o no lo que hay
dentro, porque el lector no puede saber qué guarda sin leerla — que es la definición de direccionar.
Ése es el término no plano reapareciendo.

**Lo que cuesta una ventana, medido, media de seis:**

| N | dibujables | CSR lee | LCA lee | teselas CSR | teselas LCA | sobrelectura CSR | LCA |
|---|---|---|---|---|---|---|---|
| 200.000 | 184.282 | 347.975 | 403.848 | 11,8 | 29,7 | 1,95× | 2,29× |
| 1.000.000 | 133.301 | 373.789 | 556.267 | 14,0 | 39,2 | 2,87× | 4,26× |
| 5.000.000 | 122.556 | 345.988 | 1.659.809 | 13,0 | 44,3 | 2,89× | 13,84× |
| 10.000.000 | 174.583 | 452.979 | 2.699.960 | 17,2 | 60,7 | 2,59× | 15,86× |

`dibujables` no es plano porque la ventana se dimensiona por rango en el paso 0 y luego se desliza
hacia zonas de otra densidad; lo que compara la tabla son las razones, no las columnas absolutas.

**CSR es plano y LCA no.** Lo que lee CSR va de 346k a 453k aristas mientras el corpus se multiplica
por cincuenta; lo que lee LCA va de 404k a 2,70M. Y LCA pierde también en peticiones, entre 2,5× y
3,5× las teselas — **dominado en las dos curvas**, el mismo veredicto que se le acaba de dar a
122.880.

**Por qué, en una tabla.** La primera ventana a cinco millones, nivel a nivel:

| nivel | teselas traídas | aristas dentro | de ellas dibujables |
|---|---|---|---|
| 0 | 14 | 318.043 | 108.036 |
| 1–2 | 15 | 31.048 | 5.793 |
| 3–8 | 19 | 511.345 | 2.422 |
| 9–11 | 4 | 977.000 | 3.852 |

La medición se comprueba a sí misma: la columna de dibujables suma exactamente las 120.103 del paso 0.

Las cuatro teselas altas mueven **el 53 % de todo lo que baja el lector LCA y dibujan el 0,39 % de
lo que contienen**. No es mala suerte del corpus: **cerca de la raíz al árbol no le queda ramificación
con la que podar**. A cinco millones sólo existen tres teselas de nivel 9, dos de nivel 10 y una de
nivel 11, así que una ventana cualquiera toca casi todas. La raíz a diez millones son 621.102 aristas
de las que la ventana dibuja **1.170**.

Agrupar los niveles de dos en dos —el cuadrante, que es lo natural sobre Morton— no cambia el
veredicto: funde filas adyacentes de esa tabla, no mueve ninguna arista hacia abajo. El techo no baja.

**Y CSR no necesita una segunda petición.** Toda arista dibujable tiene su origen en la ventana, luego
en una hoja tocada: leer las hojas de la ventana es un **superconjunto exacto** de lo dibujable, con
1,1–1,3× de sobrelectura a granularidad de tramo y 1,9–2,9× a granularidad de tesela.

#### 3.3. No hay árbol: hay una función de direccionamiento

Un árbol de teselas sólo tendría razón de ser para **contener** algo que no cabe en una hoja. El único
candidato era la arista que cruza teselas, y §3.2 mide que subirla cuesta más de lo que ahorra. Sin
ese contenido los nodos interiores están vacíos: **no hay nada que materializar**.

Lo que queda es aritmética. La tesela de un vértice es `dense_id >> 12`; la de su antepasado a nivel
`L`, `dense_id >> (12+L)`; y el lector computa todas las URL antes de emitir la primera. El índice no
se descubre, se calcula — y con eso desaparecen los `3·chunks + 1` HEAD y la lectura de footer de
16 kB por chunk *del corpus* que hoy paga el lector, se necesite o no.

**Y la pirámide de LOD no es ese árbol.** §1 ya lo dice: «el LOD no es filtrar, es leer otra
relación». Cada nivel de agregación es **otra relación**, con su propio espacio de `dense_id`, su
propio renumerado Morton y su propio teselado plano de 4.096 filas. No es un árbol de k niveles: son
**k teselados planos**, y dentro de cada uno la dirección es un desplazamiento.

Lo que emite el emisor, entonces, y nada más: por nivel de LOD, teselas de vértices por rango de
`dense_id` y teselas de aristas por rango de `dense_id` **de origen**. Con eso §2 y §3 dejan de
contradecirse: el CSR que GraphAr dejó como vocabulario es el que gobierna, y era el que ya estaba
escrito.

Las columnas se disponen para subir a GPU sin transformar —`x` e `y` como arrays separados—, lo que
elimina por construcción el interleave que hoy hace `arrays()` en el hilo principal.

#### 3.4. `dense_id` es una dirección; la identidad es el IRI del sujeto

Un id aplanado sólo funciona si el espacio de ids **es** el orden espacial, y eso es lo que hace el
renumerado Morton. La consecuencia es que **rehacer la maquetación renumera todo**: `dense_id` no
puede ser también la identidad. La identidad estable es el **IRI del sujeto**, que es lo que
`fossil-graph-schema/src/lib.rs:30-34` ya afirma —«every node's key is its subject IRI»— y lo que
incumplen dos sitios concretos: el `vertexId(type_idx, dense_id)` del lienzo y
`RESERVED_VERTEX_COLUMNS` (`fossil-graph/src/manifest.rs:26`), que esconde `dense_id` y `subject` en
la misma lista como si fueran lo mismo. Detalle en ADR-0045 §8.

**Si una tesela tiene que llevar el IRI para que el lector sepa qué dibuja, hay que decir lo que
cuesta.** De los footers a cinco millones, bytes comprimidos por fila: `subject` **8,016**, `dense_id`
4,000, `x` 2,717, `y` 2,501, `cluster_id` 0,028, `community` 0,014. Las cuatro columnas de dibujo son
9,23 B/fila; con `subject` son 17,25 — **1,87× la tesela de vértices**, y sobre la ventana entera
~+24 % de la carga ideal.

**Lo que obliga:** la medición de carga ideal —0,6–0,9 MB por ventana, plana en N— se tomó
proyectando `dense_id, x, y, community` y **sin `subject`**. Si el IRI entra en la tesela de dibujo,
esa cifra está mal y hay que rehacerla. La salida por defecto es que **no entre**: el camino del
dibujo lleva direcciones y el IRI vive en el sidecar de §3.5, que se pide cuando hay que *nombrar*
algo —selección, hover, un enlace hacia fuera— y no cuando hay que pintarlo. Eso no está medido; lo medido es lo que
costaría meterlo.

#### 3.5. Las propiedades van en un sidecar

Medido con teselas reales a cinco millones: una tesela con sólo `dense_id, x, y, community` cuesta **3
peticiones y 36,0 kB**, contra **4 peticiones y 37,0 kB** —de 68,8 kB almacenados— con las seis
columnas. **Los mismos bytes en el cable, un viaje menos y la mitad del almacenamiento.** La tesela de
dibujo lleva geometría; todo lo demás va aparte.

Y un cabo que la aritmética deja suelto, dicho como aritmética y no como medición: si la tesela **es**
el rango `[i·4096, (i+1)·4096)`, la columna `dense_id` es `i·4096 + índice de fila` y no hace falta
almacenarla — son 4,000 de los 9,23 B/fila, el 43 % de la tesela de dibujo. No está medido sobre
teselas reales, y depende de que los ids densos sean 0..V−1 sin huecos, que es cierto en los cuatro
corpus del banco y no está garantizado por nada.

#### 3.6. Lo que esto no prueba

- **Un solo corpus, de una sola forma.** Un tipo de vértice, un tipo de arista, partición plantada,
  renumerado por nuestro propio escritor. Un grafo de conocimiento con `place_after` y aristas entre
  tipos distintos no ha pasado nunca por aquí; la demo 1 de §Verificación es lo que movería esto.
- **«Dibujable» es «los dos extremos en pantalla».** Una arista con un extremo fuera tiene segmento
  visible y ninguna de las dos colocaciones la trae. Medirlo necesita la mitad CSC —`by_target.parquet`,
  que el corpus ya emite— y duplica el direccionamiento, no el argumento.
- **Seis ventanas por tamaño, no una distribución**, y todas a un zoom, cuadradas y centradas en el
  vértice mediano por `subject`. Otros zooms y otras relaciones de aspecto no están medidos.
- **La comparación es en filas, no en bytes en el cable.** Las cifras de bytes de §3.1 son de teselas
  CSR reales; las de LCA no se han escrito. Una tesela alta comprime *peor* que una hoja —sus ids
  abarcan un rango más ancho, luego delta y RLE rinden menos—, así que la cifra de LCA es, si acaso,
  optimista.
- **No dice si la jerarquía de agregación debe seguir siendo la de Louvain.** Ese es el coste O(V) que
  ADR-0045 pone sobre la mesa, y sigue abierto: aquí sólo está medido que el árbol de *teselas* no
  existe, no qué produce los niveles de *agregación*.

Detalle en `kanzo-ui/BENCHMARKS.md`, en `kanzo-ui/.planning/GRAPH-ROADMAP.md` y en ADR-0045 §8. El
guion que produce las tablas de §3.2 y §3.4 es `spikes/lca-vs-csr/run.sh <dir-del-corpus>`.

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

**Corregido el 2026-08-05 con §3: dos partidas de esa lista eran consecuencia de «una tesela es una
comunidad» y se invierten al caer.**

- **El renumerado Morton se queda, y pasa a ser el cimiento.** Estaba listado fuera porque una tesela
  iba a ser una comunidad y el orden de los ids daba igual. Con §3 la dirección de una tesela *es*
  `dense_id >> 12` sobre ese orden: sin el renumerado no hay direccionamiento. Lo que sí sale es su
  **emisión de chunks de 122.880**, que §3.1 retira.
- **El índice no se escribe.** §3.3 lo sustituye por aritmética. Dentro quedan el emisor y el almacén.

**El neto no se vuelve a derivar aquí**, porque estaría desactualizado antes de que exista el emisor.
Lo que sí se puede decir es que ya no es −1.400: la partida mayor de las que salían se queda.

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
fino: que el teselado salga igual sobre posiciones dadas que sobre calculadas. Con §3.3 la pregunta
se afila — no hay árbol que comparar, hay un renumerado Morton, así que lo que se comprueba es que
las posiciones dadas produzcan un orden con la misma localidad. Si no, la maquetación y el
direccionamiento están acoplados.

**Larger-than-RAM, al final**, porque es la más cara. Es **la afirmación central de la arquitectura y
no se ha probado nunca**: el corpus mayor son 5M ≈ 100 MB. La métrica que decide no es el pan sino el
**pico de heap**, que debe quedarse plano. Y el riesgo no está donde miramos: Louvain corre en memoria
sobre el grafo entero, y el generador ya reventó contra el tope de 512 MB de string de V8 a 35M
aristas. **Que fossil no pueda escribir un corpus larger-than-RAM sería un hallazgo tan importante
como cualquiera de lectura.**

Se reutiliza el arnés existente —retención, chunks tocados, pan a zoom constante, sonda de solape de
conexiones— y faltaba una métrica: **bytes descargados por paneo**, que es lo único que justifica las
teselas frente a un fichero único. **Medida el 2026-08-05** y es la que fija la unidad en §3.1: el
corpus tal cual está cuesta 2,4 / 5,4 / 12,1 / 13,1 MB y 17 / 55 / 208 / 376 peticiones por ventana a
200k / 1M / 5M / 10M, contra una carga ideal plana de 0,6–0,9 MB. Lo que crece con N no son los bytes
útiles: son metadatos, `3·chunks + 1` HEAD y un footer de 16 kB por chunk *del corpus*, se necesite o
no. Eso es lo que §3.3 quita.
