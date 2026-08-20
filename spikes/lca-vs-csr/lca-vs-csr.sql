/*
 * Dónde vive una arista: en la tesela de su origen (CSR) o en la tesela más profunda que contiene
 * ambos extremos (LCA). El modelo de teselado afirmaba lo primero en un sitio y lo segundo en
 * otro; esto lo mide en vez de discutirlo, sobre un corpus que ya existe y sin escribir un emisor.
 *
 * Lo hace computable la definición de tesela: si una tesela es un rango fijo de 4.096 filas de `dense_id` y
 * `dense_id` está renumerado en orden Morton, entonces la tesela de un vértice es `dense_id >> 12`
 * y el antepasado común más bajo de dos vértices es el prefijo común de sus ids — un XOR y un
 * conteo de ceros a la izquierda. Nivel 0 significa «los dos extremos en la misma tesela hoja».
 *
 * La ventana es la del arnés de kanzo-ui (`corpus/measure-requests.mjs`), copiada a propósito para
 * que las cifras se puedan poner al lado de las suyas: centro por `subject` —nunca por `dense_id`,
 * que lo asigna la maquetación—, cuadrado del menor lado que contiene exactamente k vértices, y
 * seis pasos de medio ancho hacia el lado con más sitio.
 *
 * Uso:  ./run.sh <dir-del-corpus> [k]
 * El corpus es cualquiera de kanzo-ui/docs/public/bench/{200000,1000000,5000000,10000000}.
 */

CREATE OR REPLACE TEMP TABLE v AS
  SELECT dense_id, subject, x, y
  FROM read_parquet(getvariable('corpus') || '/vertex/Node/chunk*.parquet');
CREATE OR REPLACE TEMP TABLE e AS
  SELECT CAST(src_dense AS BIGINT) AS s, CAST(dst_dense AS BIGINT) AS d
  FROM read_parquet(getvariable('corpus') || '/edge/Node_linksTo_Node/by_source.parquet');

-- Profundidad del árbol implícito: la tesela raíz es la que todos los ids comparten.
CREATE OR REPLACE TEMP TABLE depth AS
  SELECT CAST(floor(log2(max(dense_id) >> 12)) AS INT) + 1 AS dp FROM v;

-- Nivel LCA de cada arista, y la tesela en que la pondría esa colocación.
CREATE OR REPLACE TEMP TABLE elca AS
  SELECT s, d,
         CASE WHEN xor(s >> 12, d >> 12) = 0 THEN 0
              ELSE CAST(floor(log2(xor(s >> 12, d >> 12))) AS INT) + 1 END AS lvl
  FROM e;
CREATE OR REPLACE TEMP TABLE etile AS SELECT s, d, lvl, (s >> 12) >> lvl AS tile FROM elca;

SELECT (SELECT count(*) FROM v) AS vertices,
       (SELECT count(*) FROM e) AS edges,
       (SELECT (max(dense_id) >> 12) + 1 FROM v) AS leaf_tiles,
       (SELECT dp FROM depth) AS depth;

-- 1. Reparto global. La última fila es la raíz: lo que TODA ventana paga bajo LCA, se dibuje o no.
SELECT lvl, count(*) AS edges,
       round(100.0 * count(*) / sum(count(*)) OVER (), 3) AS pct
FROM elca GROUP BY lvl ORDER BY lvl;

-- La ventana, calcada del arnés.
CREATE OR REPLACE TEMP TABLE centre AS
  WITH ranked AS (
    SELECT x, y, row_number() OVER (ORDER BY subject) - 1 AS r, count(*) OVER () AS total FROM v
  )
  SELECT x AS cx, y AS cy FROM ranked WHERE r = (total / 2)::BIGINT;
CREATE OR REPLACE TEMP TABLE window_size AS
  SELECT max(dist) AS h FROM (
    SELECT greatest(abs(v.x - c.cx), abs(v.y - c.cy)) AS dist
    FROM v, centre c ORDER BY dist LIMIT (SELECT getvariable('k')::BIGINT)
  );
CREATE OR REPLACE TEMP TABLE extent AS
  SELECT min(x) AS x0, max(x) AS x1 FROM v;
CREATE OR REPLACE TEMP TABLE pans AS
  SELECT s.j,
         c.cx + s.j * 0.5 * 2 * w.h * CASE WHEN c.cx <= (x.x0 + x.x1) / 2 THEN 1 ELSE -1 END AS px,
         c.cy AS py, w.h AS h
  FROM range(6) AS s(j), centre c, window_size w, extent x;
CREATE OR REPLACE TEMP TABLE win AS
  SELECT p.j, v.dense_id AS node
  FROM pans p JOIN v
    ON v.x BETWEEN p.px - p.h AND p.px + p.h
   AND v.y BETWEEN p.py - p.h AND p.py + p.h;

-- Las teselas hoja que toca la ventana, en la unidad de 4.096 filas que fijó la medición del
-- 2026-08-05, y todos sus antepasados. El lector LCA tiene que traerse los antepasados enteros:
-- no puede saber cuál guarda una arista suya sin leerlo, que es lo que significa direccionar.
CREATE OR REPLACE TEMP TABLE leaf AS SELECT DISTINCT j, node >> 12 AS t FROM win;
CREATE OR REPLACE TEMP TABLE anc AS
  SELECT DISTINCT j, l.lvl, t >> l.lvl AS tile
  FROM leaf, depth, range(0, 32) AS l(lvl) WHERE l.lvl <= depth.dp;

CREATE OR REPLACE TEMP TABLE per AS
SELECT p.j AS step,
       (SELECT count(*) FROM win w WHERE w.j = p.j) AS wanted,
       (SELECT count(*) FROM leaf l WHERE l.j = p.j) AS leaves,
       (SELECT count(*) FROM anc a WHERE a.j = p.j) AS anc_tiles,
       -- dibujable: los dos extremos en pantalla, que es todo lo que el lienzo puede pintar
       (SELECT count(*) FROM etile x JOIN win a ON a.j = p.j AND a.node = x.s
                                     JOIN win b ON b.j = p.j AND b.node = x.d) AS drawable,
       -- CSR: toda arista cuyo ORIGEN cae en una tesela hoja tocada. Superconjunto exacto.
       (SELECT count(*) FROM etile x JOIN leaf l ON l.j = p.j AND l.t = x.s >> 12) AS csr_read,
       -- LCA: toda arista en una hoja tocada o en cualquier antepasado suyo
       (SELECT count(*) FROM etile x JOIN anc a ON a.j = p.j AND a.lvl = x.lvl AND a.tile = x.tile)
         AS lca_read
FROM pans p ORDER BY p.j;

-- 2. Coste por ventana. `*_over` es contra lo dibujable; las teselas son la cuenta de peticiones.
SELECT step, wanted, leaves, anc_tiles, drawable, csr_read, lca_read,
       round(csr_read::DOUBLE / drawable, 3) AS csr_over,
       round(lca_read::DOUBLE / drawable, 3) AS lca_over
FROM per;
SELECT 'media' AS step, round(avg(wanted)) AS wanted,
       round(avg(leaves), 1) AS leaves, round(avg(anc_tiles), 1) AS anc_tiles,
       round(avg(drawable)) AS drawable, round(avg(csr_read)) AS csr_read,
       round(avg(lca_read)) AS lca_read,
       round(avg(csr_read::DOUBLE / drawable), 3) AS csr_over,
       round(avg(lca_read::DOUBLE / drawable), 3) AS lca_over
FROM per;

-- 3. De dónde salen los bytes del lector LCA en la primera ventana, nivel a nivel. La comprobación
-- que hace la tabla creíble: `of_them_drawable` suma exactamente `drawable` del paso 0.
SELECT a.lvl,
       count(DISTINCT a.tile) AS tiles_fetched,
       count(x.s) AS edges_in_them,
       count(x.s) FILTER (WHERE w1.node IS NOT NULL AND w2.node IS NOT NULL) AS of_them_drawable
FROM anc a
LEFT JOIN etile x ON x.lvl = a.lvl AND x.tile = a.tile
LEFT JOIN win w1 ON w1.j = 0 AND w1.node = x.s
LEFT JOIN win w2 ON w2.j = 0 AND w2.node = x.d
WHERE a.j = 0
GROUP BY a.lvl ORDER BY a.lvl;

-- 4. Lo que cuesta cada columna del vértice, de los footers. Decide si la tesela lleva el IRI.
SELECT path_in_schema AS col,
       sum(total_compressed_size) AS compressed,
       round(sum(total_compressed_size)::DOUBLE / (SELECT count(*) FROM v), 3) AS bytes_per_row
FROM parquet_metadata(getvariable('corpus') || '/vertex/Node/chunk*.parquet')
GROUP BY 1 ORDER BY compressed DESC;
