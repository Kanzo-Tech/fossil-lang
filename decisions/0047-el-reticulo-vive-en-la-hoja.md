# ADR 0047: El retículo de primitivas vive en la hoja, y `DataType` era él mismo

**Date:** 2026-08-07
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Ejecuta la fase F1 de ADR-0046 §Consecuencias («obliga a que `Primitive` baje a un crate
hoja»). Supersede el puente de cadenas de ADR-0007 §"Datatype catalog" y el campo `primitive:
SmolStr` de ADR-0037. El conteo de tablas es de `apps/docs/content/docs/characteristics/types.mdx`,
verificado en el árbol el 2026-08-07.

## Contexto

`Primitive` vivía en `fossil-hir` y los descriptores no pueden depender del HIR: sería un ciclo.
ADR-0007 resolvió esa dirección con una cadena — `datatype_to_primitive_name` devolvía el **nombre**
canónico de la variante, y `fossil-hir::infer::primitive_from_name` lo volvía a convertir en el
extremo consumidor. ADR-0037 heredó la misma decisión para el descriptor del host:
`InferredColumn.primitive` era un `SmolStr` cuya invariante estaba escrita en un comentario.

El coste, medido: **siete implementaciones de una tabla**, dos de ellas en el mismo fichero, y una
(`is_canonical_primitive_name`) que existía sólo para afirmar que coincidía con otra. Un valor fuera
del retículo no era un error: se convertía en `String` y producía un diagnóstico
`D-INFERRED-UNKNOWN-DATATYPE` tres crates más allá, hablando de una columna en vez del valor.

Y había una octava copia que nadie contó: `fossil-graph-schema::DataType` — nueve variantes, la misma
`from_xsd_iri`, y su propio doc-comment declarando que era «1:1 con el `Primitive` de fossil».
`fossil-df` mantenía las dos funciones identidad que traducían entre ambas.

## Decisión

**El retículo baja a `fossil-graph-schema`, y `DataType` desaparece dentro de él.** No es un crate
nuevo: es el que ya tenía el retículo con otro nombre, no depende más que de `serde`, y tanto el HIR
como los descriptores pueden depender de él sin ciclo.

1. `TyKind::Primitive` lleva `fossil_graph_schema::Primitive`. Salsa no necesita nada: el `derive` de
   `Update` cae al camino de comparación por `PartialEq` para un tipo que no implementa `Update`.
2. `InferredColumn.primitive` y `CsvwDescriptor::type_for_column` hablan el enum. **No hay
   reexportación desde `fossil-hir`**: quien nombre el retículo depende de la hoja y lo dice en su
   `Cargo.toml`.
3. Queda **una función por dirección**, ambas en el retículo: `from_xsd_iri` — que acepta el IRI
   completo, la forma `xsd:` y el nombre desnudo de CSVW, las tres deletreos que el árbol lleva — y
   `to_xsd_iri`. `primitive_to_graphar` y `duckdb_type_to_fossil_primitive` **se quedan donde
   están**: son vocabularios de un materializador, no copias del retículo.
4. **El formato de cable cambia**: el host envía `"integer"`, no `"Integer"`. Es el `snake_case` que
   `fossil-graph-schema` ya serializaba; ahora hay un solo deletreo para las dos fronteras.
   `@fossil-lang/{introspect,wasm}` lo siguen.

## Consecuencias

**Lo que se vuelve más fácil.** Una primitiva fuera del retículo es un error de deserialización que
nombra el valor, en el punto donde llega el JSON. `D-INFERRED-UNKNOWN-DATATYPE` se borra por
imposible, no por descuido. Y F2 y F4 pueden hacer cruzar el retículo tres fronteras sin multiplicar
los sitios donde divergir, que es la razón por la que esto iba primero.

**Lo que se paga.** Un host que ya enviaba `"Integer"` deja de compilar contra el nuevo cable. No hay
alias ni periodo de gracia: es la regla del repo y el paquete no está publicado. `AnyURI` se escribe
`AnyUri` — el deletreo que `fossil-graph-schema` ya usaba, y el que la guía de estilo de Rust pide.

**Lo que queda abierto.** Seis crates ganan una arista hacia `fossil-graph-schema`. Es la arista
correcta —todos hablan el retículo— pero el nombre del crate ya no describe lo que contiene: no es
sólo el esquema de un grafo, es el contrato de datos que el compilador y el corpus comparten. F8
renombra crates cuando un documento lo fuerce; éste es ese documento para éste.
