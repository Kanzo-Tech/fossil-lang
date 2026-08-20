# El plan de construcción

**Reescrito 2026-08-11**, después de auditar el corpus entero. **Ampliado 2026-08-14**: absorbe las
fases F1–F8, que vivían en un segundo documento fuera del repo. **Esto es el orden de construirlo**,
y es el único sitio donde está.

> **Sobre los `ADR-00NN` que aparecen aquí.** `decisions/` está borrado. Los números que quedan en
> este documento son **etiquetas de un suceso**, no punteros a un fichero: cada frase que nombra uno
> dice a continuación lo que aquella decisión decía, y el argumento vive en `/docs/design`. Un
> número aquí no es una instrucción de ir a leer nada, y no hay nada que leer.

> **Por qué hay uno y no dos.** Las fases F describían el mismo compilador desde el otro extremo —
> los tipos, Salsa, el escritor de Parquet, los crates — y estaban en `~/.claude/plans/`, fuera del
> árbol y sin guardia. Medido el 2026-08-14: **el trabajo de superficie había cerrado F1, F2 y media
> F5 sin que ninguno de los dos documentos lo supiera**, y los dos habían acumulado afirmaciones que
> el otro ya había invalidado. Dos planes sobre un compilador es el mismo defecto que el paso 9
> existe para cerrar, con la agravante de que uno de ellos no estaba versionado.

Cada paso dice **qué lo desbloquea** y **qué prueba que está hecho**, porque este repo tiene tres
casos documentados de dar algo por terminado porque estaba escrito: la fase 3 marcada `8/8` con el
checker que no corría (82 días), la fase 13 `6/6` de tres cosas que ninguna existía (72 días), y
once operadores tipados de los que cuatro eran alcanzables.

---

## Lo que se decidió el 2026-08-11, y gobierna

1. **fossil no se describe como «interpretado».** No hay paso de build ni artefacto que el usuario
   maneje; eso es lo que se dice. Mantiene en pie el argumento de ADR-0059 contra `::`, que se apoya
   en *«un programa fossil no ejecuta nada»*.
2. **El polimorfismo vive sólo en la biblioteca.** Firmas genéricas sobre tipo de elemento y de
   fila; el usuario no declara funciones. Zanja `stdlib.md` (`forall T.`) contra `type-system.md` §2
   (*«functions are monomorphic»*).
3. **Nombrar un documento de forma es obligatorio.** Una clave desnuda saca su nombre de un
   predicado que la forma declara, así que sin forma no se puede escribir ni una propiedad.
   **Deroga la 5ª enmienda de ADR-0057** y devuelve sentido a ADR-0055 §6.
4. **MCP lee corpus.** El lado máquina del compilador son los diagnósticos y el `llms.txt`.
   `fossil-mcp` sirve los seis verbos contra un GraphAr ya escrito: es del lado grafo, no del
   lenguaje, y el nombre lo desmiente.
5. **La convención del corpus es un documento con guardias ejecutables**, no un crate compartido
   (responde la pregunta abierta de ADR-0045 §1, y desbloquea los veredictos de `fossil-sinks` y
   `fossil-graph-schema`). Lo empuja el ítem 5 del roadmap: el payload de una tesela puede no ser
   Parquet, y un crate que modele *el* manifiesto asume un solo formato por construcción.
6. **Una arista es una referencia.** RDF es de mundo abierto: un `buyer` puede apuntar a una
   `Person` que ningún mapeo emitió, y ADR-0058 garantiza que el IRI esté bien formado igualmente.
   Sin comprobación, y **dicho en la documentación** en vez de callado.
7. **`|>` muere.** La llamada por miembro es la grafía. Deroga ADR-0050 §2.
8. **La documentación se parte en dos**, porque son dos productos y la costura es el corpus en
   disco, no una API.

---

## La regla que gobierna esta fase (2026-08-12)

**No hay grafías intermedias.** El destino es `grammar.bnf` — que ya está escrita entera contra las
ocho decisiones — y el criterio de «hecho» son los **18 programas de `apps/docs/programs/`**, que la
propia gramática nombra como conjunto de conformidad en su cabecera. Nada se parchea a una forma que
vayamos a borrar: si un fixture no se puede escribir en la grafía final, lo que falta es la
producción, y esa es la tarea. Una reescritura del corpus, al final, y una sola.

Corolario operativo: **el árbol estará rojo mientras dure**, y eso es correcto. Perseguir el verde
paso a paso es lo que produjo las tres fases dadas por terminadas porque estaban escritas.

## Dónde se quedó (2026-08-12, medido, no recordado)

**Falso lo que decía la versión anterior de esta sección:** los cinco crates **sí compilan**.
`cargo check --workspace --all-targets` es verde; los dos agentes que cayeron por límite de sesión
dejaron sus arreglos en el árbol. Lo que hay rojo son **~35 tests en 20 binarios de 7 crates**
(`fossil-cli` 7, `fossil-df` 13, `fossil-df-wasm` 3, `fossil-engine` 2, `fossil-wasm` 2,
`fossil-ide` 4, `fossil-mir` 3), y **por una sola causa**: sus programas no nombran documento de
forma, así que `TypeckOutput.predicates` está vacío y las propiedades no compilan
(`Plan("the mapping did not compile")`) o compilan sin `rdf_uri`. No son 3 en `lower_pg`.

**Verde y cierto:** el corte (paso 0); el vocabulario neutro y la tabla de `fn`; las **dos** webs de
documentación (`@fossil-lang/docs` 48 tests + 87 páginas, `@fossil-lang/corpus` 15 guardias + 16
páginas), verificadas en limpio; `grammar.bnf` a 588 líneas; `cargo deny` verde.

**Rojo y no contado hasta ahora:** `cargo fmt --all -- --check` falla, tres diffs en
`crates/fossil-engine/src/documents.rs`.

### Lo que la auditoría del 2026-08-12 encontró y hay que arrastrar

- **Los snapshots no se aceptaron deprisa: no se revisaron.** Diez de los trece de `fossil-hir`
  llevan `assertion_line:` dentro, y `insta::MetaData::trim_for_persistence` lo borra
  **incondicionalmente** en `save` — por el que pasan todas las rutas soportadas, `cargo insta
  accept` incluida. Sólo los `.snap.new` lo conservan. Es prueba de que se movieron los pending a
  mano. No hay opción que desactivar; hace falta **una guardia**: un test que recorra `**/*.snap` y
  falle si alguno contiene `assertion_line:`.
- **El arnés de `fossil-hir/tests/diagnostic_corpus.rs` no puede resolver un documento de forma**,
  ni poniéndole un `.shex` al lado: su `new_db()` usa `NativeSystem`, cuya `shape_decoders()` es
  `&[]`, y `decoded_document` resuelve por `file_at`, que lee el registro de inputs de Salsa y no el
  disco — y `run_csvw_fixture` nunca llama a `register_file`. Arreglar los siete fixtures produciría
  **siete verdes falsos**. Primero el arnés.
- **`render_split_suggestion` recibe en producción el nombre del mapeo donde va la cláusula `from`**
  (`check.rs:1338`), y emite `Contact1 : ex:Contact from Contact`. El snapshot no lo ve porque el
  test pasa otro argumento. El valor correcto es `Checker::source_binding_name()` (`check.rs:1397`).
- **Los spans `0..0` son exactamente tres**: `check.rs` 184, 901 y 945 (más 1324, preexistente).
  Todo ADR-0058 (`body.rs:249-291`) y todos los rechazos de clave de 0059 en el lowering subrayan
  bien. Y como el marco por defecto es `MappingRelative`, `rebase_to_file` convierte `0..0` en
  `base..base`: no es que no subraye, es que **apunta a un sitio plausible y equivocado**.
- **`decisions/` es dependencia dura de la build de `apps/docs`, por dos caminos**: `lib/decisions.ts:30`
  hace `readdirSync` sin `try` y lo consume una página prerenderizada; y `content.test.ts:221` exige
  que los `decidedBy` de **9 páginas** existan en disco. Borrar `decisions/` sin la cirugía previa
  pone la build roja dos veces. Los cuatro `.md` de la raíz sí se pueden borrar sin tocar nada;
  `grammar.bnf` **no** (transcluido entero en `book/grammar.mdx:33`).
- **Nadie compila los 18 programas.** Lo único que se comprueba es que el fichero y su `// #region`
  existan. `grammar.bnf:24` ya los declara conjunto de conformidad y ese conjunto no lo ejecuta
  nadie.
- **`resolve_target_shape` ya NO se traga los fallos, y ahora tampoco las nombra**: cuatro de las
  cinco variantes de `TargetShapeError` se borraron el 19 tras probarlas inalcanzables — la ligadura
  posicional hace que todo falle antes, en el binding. Queda `NoDocument`, igual de muerta, esperando
  a que se pueda colapsar el `Result` en `fossil-ide`.
- **`DefMap::lookup_prefix` no existe**: borrada, lápida en `def_map.rs:138-141`.

### El agujero entre el paso 4 y el paso 7, que este plan no veía

Matar `subject_skeletons` no migró la capacidad de aristas: **la perdió**, y la lápida que lo justifica
es falsa. Dos bloqueos medidos ejecutando, no leyendo:

**A · Una arista no type-checkea, y es insatisfacible.** `shapes.rs::expected_value_ty` espera
`TyKind::Iri` para un predicado cuyo rango es una forma; `infer.rs::record_from_shape:595-600` tipa
esa columna como `Primitive::String`; y `check.rs::subtypes` dice que `String` no es subtipo de `Iri`
ni de `AnyUri`. Una plantilla IRI en posición de valor sale también `String`, porque el flag
`subject_position` de `check.rs:823` sólo se enciende para `PropertyKey::Subject`. **Nada en el árbol
produce un `Iri` fuera de la posición de sujeto.** Y no es un aviso: `typecheck_mapping` devuelve
`Err`, `lower_to_mir_pg` envenena, y el run entero falla.

Corrección al inventario de trampas de este documento: dice que `value_ty: None` no significa
«cualquier valor», y es cierto. Le falta la que sale más cara — **`value_ty: Some(Iri)`, el caso de
arista, es insatisfacible**.

**B · ~~El documento de forma nunca llega al ejecutor.~~ CERRADO el 2026-08-12.**
`resolve_output_descriptor` ya lee `def_map.output_shape_document()` primero — `lib.rs:372`, con su
docblock describiendo el arreglo. El agujero de `ACCEPT_ALL_DEFAULT` y cero aristas está cerrado.

**B′ · Pero el ejecutor no sabe leer lo que el checker leyó.** `fossil-engine/src/lib.rs:427`,
`read_output_shape`, parsea con `ShExDescriptor::from_reader`, que es **ShExJ (JSON) y sólo ShExJ**.
Los `.shex` del corpus son **ShExC**. El camino del checker va por `decode_shex` → `from_shex_source`,
que autodetecta los dos. Así que un documento que **type-checkea** hace fallar el `run`. Ninguno de
los dos lados está mal por separado, y así es como sobrevivió.

**C · `io.shacl` no lo decodifica nadie.** El programa `catalogue` nombra `io.shacl("catalogue.ttl")`
y la única fila de decodificador instalada reclama `shex`/`shexj`/`shexc`. `Product` no liga forma, y
por la regla 3 ninguna propiedad de ese mapeo es escribible. Es el único de los 18 con esta causa.

Dicho de otro modo: **la regla 3 gobierna el checker y no el ejecutor.** El documento se lee para
comprobar y se ignora —o se lee con otro parser— para ejecutar.

**D · El corpus pierde 49 de sus 74 propiedades en silencio.** Predicho leyendo el lowering, no
ejecutando: los 25 encabezados de mapeo de los 18 programas exigen todavía un CURIE
(`lower.rs:543-548`, `SHAPE_EXPR > IRI_EXPR` con ≥2 IDENTs) y `lower_property` devuelve `None` para
una clave desnuda, que `body.rs:207` se salta con un `if let` pelado — sin diagnóstico y sin
contador. Sobreviven los 25 `@subject` y **desaparecen 49 sin una palabra**. Ésa es la cifra que
convierte un arnés ingenuo en verde perdiendo el programa.

### El fan-out, que no se toca

**MEDIDO, y salió gratis.** `MAX_REEXECUTIONS` se queda en **18** y `MAX_PER_MAPPING_FAN_OUT` en
**1**. Las dos dependencias nuevas son *file-keyed*: `shape_document` se clava por el `SourceFile`
**del documento**, así que diez mapeos comparten una decodificación y editar el PROGRAMA la
reejecuta **cero** veces. El coste está sólo en la dirección contraria, y es el precio de ser
correcto. **Reverificado el 2026-08-12** tras poner los fixtures al día: las decodificaciones tras
editar el PROGRAMA siguen en delta **0**, tras editar el DOCUMENTO en **+1**, y las reejecuciones
tras editar un cuerpo con diez mapeos siguen en **18**. Ninguna constante se mueve.

**Pero el hueco de la evidencia, dicho por quien la reverificó, y es real:**

- **Ningún test demuestra que N mapeos comparten UNA decodificación.** El fixture de `documents.rs`
  tiene **un** mapeo, así que 0 y +1 son compatibles con file-keyed *y* con per-mapping. Hoy el
  argumento descansa en la firma de la consulta —`shape_document(db, doc: SourceFile)`, clavada sólo
  por el `SourceFile` del documento— y no en un número. Merece un test con varios mapeos.
- **El fixture de diez mapeos no nombra documento de forma**, así que `resolve_target_shape` sale por
  `NoDocument` sin tocar `file_at` ni `shape_document`. **18 es verde y no dice nada sobre la ruta
  nueva.** El propio docblock del test lo avisa.
- Y los seis tests que este documento citaba como prueba del fan-out **no prueban
  `MAX_REEXECUTIONS`**: las constantes viven en `crates/fossil-hir/tests/invalidation_regression.rs`
  y los seis prueban la costura del documento de forma. Eran dos cosas distintas contadas como una.

## El estado, medido

| | |
|---|---|
| Ficheros `.fossil` versionados | **74** — no 68, no 83, no 35. Reparto: `packages/examples/src` 36, `fossil-syntax/tests` 20, `fossil-hir/tests` 14, sueltos 4 |
| Crates | **24**. ADR-0046 §7 apunta a 5–8 |
| Operadores del álgebra | **14** — `operator-algebra.md` dice «nueve más dos refinamientos» |
| Entradas de la stdlib | **53** — ADR-0048 dice 56 |
| Verbos del grafo | **6** — ADR-0039 dice 14 |
| Enmiendas de ADR-0057 | **10** — el índice dice siete |
| Referencias muertas en la prosa versionada | **159** |
| Citas `ADR-0050` que resuelven al registro equivocado | **18 de 20** (16 → 0053, 2 → 0052) |

---

## Hecho

| | commit |
|---|---|
| Referencia cualificada `users.name` | `f9ee991` |
| Interpolación `"{expr}"`, y el parser de plantillas sale de MIR | `3de025f` |
| Los cuatro paquetes de UI fuera (7.865 borrados) | `873cbc0` |
| Poda 1: `$VAR`, `<<s p o>>` | `ee6fdef` |
| Los 6 verbos RDF de la stdlib y 2 tipos del retículo | `4213e74` |
| ADR-0058, ADR-0059 | `8133424`, `62fe56c` |
| `timeout_ms`, muerto, y su comentario que mentía | `329d7ef` |
| **`fossil-mir` deja de conocer ShEx** | `0e6898d` |
| Poda 2: `in`, `&`, anotaciones, registros, `use`, `T_COLON` | `62ed84f` |
| `temp_dir` fijo del CLI | `6b597ea` |
| **El vocabulario neutro y la costura de salsa** | *en el árbol* |

`a0d9bfa` construyó `@subject(iri = …)` y ADR-0058 lo mató dos horas después. No cuenta como hecho:
cuenta como deuda.

---

## El árbol

```
┌─ 0 · CORTE DEL DESCRIPTOR DE SALIDA ─────────────────── HECHO
├─ 1 · CLAVES DESNUDAS   name = …  ────────────────────── HECHO
├─ 4 · IDENTIDAD  @subject = expr  ────────────────────── HECHO
├─ 2 · CABECERA   Users : Person from Adults ─────────── HECHO
├─ 3 · FUERA `prefix`, el CURIE, ABS_IRI, `${ex:}` ───── HECHO
├─ 5 · TIPOS DE ENTRADA CON NOMBRE  User := io.csv(…) ── HECHO
├─ 6 · VERBOS COMO CATÁLOGO  (+ murió `|>`) ──────────── HECHO
└─ 7 · `:=` para `type`; ARISTA; `@rename` ───────────── HECHO
                    ▼
        8 · LOS PROGRAMAS DE LA DOCUMENTACIÓN SON EL CORPUS   ← AQUÍ
            los 23 se compilan y se guarda su salida o su
            diagnóstico. 104 tests rojos, en DOS montones.
            (el paso 9 se adelantó: no dependía de éste)
                    ▼
        9 · UNA SOLA REFERENCIA, Y ES LA BASELINE ─────────── HECHO
            `decisions/` y `.planning/` borrados, 173 citas sustituidas
                    ▼
     ┌──────────────┴──────────────┐
     ▼                             ▼
 F3 · LA CACHÉ              F4 · LA FORMA ES EL CONTRATO
 F6 · SALSA SE ENCOGE       F5 · EL PIPELINE, medido
 F7 · EL ESCRITOR             (los tres se miden CONTRA el corpus)
     └──────────────┬──────────────┘
                    ▼
        F8 · LOS CRATES, Y SON DOS ÁRBOLES
```

**Ruta crítica:** ~~2 → 3 → 5 → 7 → 6~~ → **8** → 9 → F4/F5 → F8.
F3, F6 y F7 no tocan la gramática ni el corpus y corren en paralelo desde ya.

**El paso 7 subió a la ruta crítica el 2026-08-12**, y este árbol lo dibujaba como rama lateral.
El motivo: la **única** prueba end-to-end que existe —`fossil run` real → Parquet → DuckDB, las doce
comprobaciones de `crates/fossil-engine/tests/conformance.rs`— está bloqueada por el constructor de
aristas. Sin él no hay evidencia de artefacto de ninguna clase, porque una arista es lo que hace que
haya `edge/` que comprobar. Medido: con la sintaxis al día y un `.shex` real registrado, el programa
parsea, resuelve la forma, baja, y muere en el checker con `expected Iri, got String`.

**Corren en paralelo, sin tocar la gramática:** la poda de lo ya muerto (§«Ya muerto»), los tres
spans `0..0`, el `from` de `render_split_suggestion`, la guardia de `assertion_line`, el arnés de
`diagnostic_corpus`, y la cirugía de `apps/docs` que desengancha `decisions/`.

---

## Las fases F, absorbidas y medidas el 2026-08-14

Venían de `~/.claude/plans/`, gobernadas por ADR-0046 «un núcleo y carcasas finas» — cuyo contenido
ya vive en `/docs/architecture`, así que sobrevive al paso 9. **El estado no es el que decían.**

| | qué pedía | medido el 14 |
|---|---|---|
| **F1** | `Primitive` a un crate hoja | ✅ vive en `fossil-graph-schema`; `InferredColumn.primitive` es `Primitive` |
| **F2** | call, comparison, conditional, pipeline en el HIR | ✅ `HirExpr` pasó de **4 variantes a 13**; `UNARY_EXPR` desciende (`lower.rs:1663`) |
| **F3** | la caché existe de verdad | ◐ el `Providers`-de-rustc **ya está** (§4 del F-plan = el registro de proveedores); `freshness_token` real en `fossil-engine`; falta la clave por URI |
| **F4** | el descriptor real llega al typecheck | ✅ **el criterio está verde, medido el 15**; `lib.rs:470` es el camino de `run`, no el del typecheck — ver abajo. Quedan tres restos, ninguno bloquea |
| **F5** | el pipeline compila y `rewrite.rs` no existe | ◐ `rewrite.rs` borrado; `lower_source_pipe` escrito; los dos e2e rojos por fixture |
| **F6** | Salsa fuera de `engine` y `df-wasm` | ❌ `fossil-engine/src/system.rs:111` y `fossil-df-wasm/src/lib.rs:210` siguen construyendo un `FossilDb` por llamada |
| **F7** | `arrow-rs` en vez de `COPY` | ⊘ **revertida el 15, y su premisa era falsa el 16**: el escritor GraphAr **ya era `arrow-rs`** (`fossil-df/src/files.rs`, uno solo, nativo y navegador); `COPY` sólo sobrevive en el post-paso de layout. Y donde sobrevive, medido, `arrow-rs` no compensa — ver abajo |
| **F8** | los crates | ⛔ bloqueada a propósito, y **redibujada**: ver abajo |

**F1, F2 y media F5 las cerró el trabajo de superficie**, que no sabía que las estaba cerrando. Ésa
es la evidencia de que eran un plan y no dos.

### Dos correcciones que sólo se ven leyendo los dos juntos

- **El criterio de F5 estaba escrito en sintaxis muerta.** Decía *«hecho cuando `users |> where(.edad
  >= 18)` compila»*, y `|>` es una tumba en `grammar.bnf`: *«`a |> f()` y `a.f()` eran una idea con
  dos grafías, y ganó el punto»*. **El criterio nuevo:** `users.where(User.edad >= 18)` compila, tipa
  y produce el corpus correcto, y `rewrite.rs` no existe. Los dos tests e2e rojos escriben `|>`, que
  es la misma causa y se arregla en el paso 8.
- **F6 y F7 no se movieron, pero sus citas sí.** ADR-0046 citaba `system.rs:60-65`; hoy es la 111. El
  problema intacto, la referencia rota — que es exactamente el modo de fallo que el paso 9 ataca.

### F8 · Los crates, y son DOS árboles

**Decidido el 2026-08-14.** ADR-0046 pedía «24 a 5–8» en un árbol. El corte por producto lo redibuja:
`apps/docs` documenta el lenguaje y `apps/corpus` el formato, y la página de arquitectura ya agrupa
`graph` / `graph-wasm` / `mcp` como *corpus* con **una sola arista al resto del árbol**, vía
`fossil-sinks`. Esa arista única es el corte. Así que F8 no es una consolidación: es **una separación
y después una consolidación por lado**, y la costura entre los dos es el corpus en disco — que es
precisamente lo que `apps/corpus/guards/` ya sabe comprobar sin fossil, sin Rust y sin pnpm.

Sigue en pie lo que el F-plan decía de F8, y sigue siendo la regla que la gobierna: **cada movimiento
tiene que estar forzado por un documento, no por una taxonomía.** `lsp-types` en un `Cargo.toml`.
`fossil-base` pierde su arista y se llama por lo que es. `fossil-run-status` se disuelve en la
carcasa. `fossil-mcp` está mal colocado y mal llamado — es la cara IA del lado grafo, y con el corte
en dos árboles eso deja de ser deuda y pasa a ser su sitio.

---

## F8, reescrita el 2026-08-19: el destino primero

**La dirección, en una frase:** *fossil es un compilador que se consume como biblioteca WASM, y el
corpus es un formato con una API ligera que cualquiera puede leer sin conocer sus tripas; el CLI es
un cliente más, no la interfaz.*

**El destino son tres grupos, y se nombran en producto, no en crates:**

| grupo | qué es | quién lo lee |
|---|---|---|
| **corpus** | el formato y todo lo que lo escribe y lo lee: el modelo de manifiesto, el direccionamiento, la disposición en teselas, los verbos, el ejecutor, la cara MCP | cualquiera con un corpus y una URL |
| **lenguaje** | el compilador: sintaxis, HIR, MIR, descriptores, el backend que produce el corpus | quien escribe programas fossil |
| **herramientas** | CLI, LSP, la superficie de editor, y las cáscaras WASM | quien opera fossil |

La prueba de que un crate está en el sitio correcto es una pregunta, no una taxonomía: **¿quién
tiene que entenderlo?** `fossil-runtime::graph_exec` ejecuta consultas sobre el corpus y vive dentro
de la ruta de escritura del compilador. Nadie que lea el corpus debería encontrárselo ahí, y nadie
que escriba un programa fossil debería tropezar con él. Está en el sitio equivocado, y eso basta —
no hace falta que además desbloquee una arista.

### La API del corpus, que es la pieza que faltaba

`resolveCorpus` **no** es la API de referencia. Es la capa de direccionamiento: devuelve URLs, y deja
al consumidor sabiendo qué es una tesela, qué contenedor hay, cómo pedir footers y cómo unir CSR con
CSC. Eso es exactamente lo que el traspaso pedía **no** tener que saber.

La API de referencia es ésta, y el direccionamiento queda debajo de ella, invisible:

```
const corpus = await openCorpus(url)      // una URL y un fetch, nada más

corpus.types                               // qué hay dentro
corpus.window({ x, y, w, h })              // → nodos + aristas, y si la respuesta está completa
corpus.node(id)
corpus.neighbours(ids, { depth })
```

Ni teselas, ni `dense_id`, ni Morton, ni `by_source`, ni prefijos, ni footers.

**Y para que sea de referencia y no «la librería de kanzo-ui», dos condiciones que hoy no se cumplen:**

1. **El contrato es el corpus de conformidad, no el TypeScript.** Ya existe (`apps/corpus/conformance/`,
   con `chunk_size: 64` a propósito para cazar a quien clave el desplazamiento). Falta que sea el
   contrato de *esta* API y no sólo del direccionamiento.
2. **Una segunda implementación que no comparta código con la primera.** Hoy `verify.mjs` es una
   transcripción casi línea a línea de `address.ts`: detecta deriva, que es su trabajo, pero **una
   idea equivocada compartida pasa las dos**. Y hay un vacío peor — **el escritor no está en el
   lazo**: el corpus de conformidad lo generó `guards/fixture.mjs`, no `fossil run`, así que hay dos
   *lectores* que coinciden y ningún lector comparado con el *escritor*.

Además, cinco cosas que un consumidor todavía tiene que aportar de su cosecha y que la API debe
absorber o declarar: **qué contenedor** (fichero por tesela o row-groups — tres artefactos del árbol
tienen tres defaults distintos y el ganador medido es el que fossil no sabe escribir), **cuántas
teselas** (ningún manifiesto lleva el número de vértices), **las cajas** `x`/`y`, **el vocabulario del
payload**, y **la identidad del corpus** — no hay forma de nombrar un vértice que sobreviva a un
relayout, que es el hueco 5 del traspaso y sigue sin dueño.

**Lo primero que hay que corregir es la premisa de la sección de arriba: la arista NO es una.** Son
tres, y las lleva `fossil-mcp` (`→ fossil-graph`, `→ fossil-runtime`, `→ fossil-resolver`), y
`fossil-runtime → fossil-graph` va en sentido contrario. La causa raíz es que **`fossil-runtime` son
dos crates**: `graph_exec.rs` es el ejecutor del lado corpus, `layout.rs` + `materialize.rs` son la
ruta de escritura del lenguaje.

**Y el árbol está mejor de lo que la conversación sugería.** `fossil-lineage`, `fossil-syntax`,
`fossil-graph-schema`, `fossil-mir` y el corte `fossil-ide`/`fossil-lsp` están todos forzados por algo
real y verificado. **Dos crates hacen todo el ruido:** `fossil-base` (≈1310 de 2680 líneas no son ni
el trait ni la db) y `fossil-engine` (cinco verbos sin relación que sólo comparten `open_db` — no es
hondo, es ancho). Arreglar esos dos es la mayor parte de «está todo un poco liado».

### Lo que una frontera compra aquí, y quién no paga

Tres cosas, y sólo tres: un **tripwire `cfg`**, un sitio en el **cierre del gate WASM**, y los **tipos
concretos que Salsa necesita**. Medido: el cierre es exactamente `{base, descriptors-input,
descriptors-output, df, df-wasm, graph, graph-schema, graph-wasm, hir, ide, lineage, mir, run-status,
shex, sinks, syntax, wasm}`, y su complemento es exactamente el conjunto de tripwires más
`fossil-mcp`. **`fossil-run-status` no compra ninguna de las tres**, y su razón declarada —«los hosts
dependen de este crate»— es **imposible con `publish = false`**; keasy reescribió la forma a mano, y
de ahí salió la deriva del campo `version`.

### Dónde va cada cosa, y qué se resiste

**corpus** — `fossil-sinks` (el modelo de manifiesto **es** el formato), `fossil-graph`,
`fossil-graph-wasm`, `fossil-mcp`, y **las dos mitades de `fossil-runtime` que hoy no son suyas**:
`graph_exec.rs` (lee el corpus) y `layout.rs` (lo tesela y lo ordena por Morton). `apps/corpus` y
`packages/graph` son su documentación y su API.

**lenguaje** — `fossil-syntax`, `fossil-hir`, `fossil-mir`, `fossil-df`, `fossil-descriptors-{input,output}`,
`fossil-shex`, `fossil-lineage`, y `fossil-base` una vez esté limpio. `apps/docs` es su documentación.

**herramientas** — `fossil-cli`, `fossil-lsp`, `fossil-ide`, `fossil-wasm`, `fossil-df-wasm`,
`fossil-engine`, y lo que quede de `fossil-resolver` (un renderizador de `CREATE SECRET`, que es una
preocupación de host).

**`fossil-graph-schema` es la costura** y se queda entre los dos: es el vocabulario neutro que el
compilador escribe y el corpus lee. Su encabezado dice hoy *«Two contracts, one crate»*, y eso es lo
único que hay que arreglarle.

**Lo que se resiste, y por qué:**

- **`fossil-runtime` no se puede partir sin sacar `Probe` de `fossil-base`**: es su **única** arista
  de producción al lado lenguaje, un solo `use`, y encima `Probe` hace I/O fuera de `System` y no
  está ni re-exportado.
- **`fossil-shex` no puede volver con los descriptores mientras exista `fossil-base →
  fossil-descriptors-input`**, que es **un solo método de trait** y cierra un ciclo.
- **`fossil-base` no puede llamarse substrato** mientras tenga dentro el catálogo `io.` **con la
  prosa de los errores de usuario**, una query `tracked` y la resolución de rutas `@conn`. Son ~1310
  de sus 2680 líneas, y `CLAUDE.md` prohíbe lógica de compilador ahí por su nombre.
- **`fossil-engine` no es un crate, es un saco** — cinco verbos sin relación que sólo comparten
  `open_db`. Cada uno se va con su grupo; lo que quede es la carcasa nativa.
- **`fossil-run-status` no va a ningún grupo: desaparece.** Sus tres contratos vuelven a quien los
  produce — `RunStatus` a `fossil-df`, `ProviderInfo` y `SourceRefInfo` a `fossil-lineage`, que hoy
  se los importa de vuelta a sí mismo. Ninguno se convierte en método del LSP: `run` no es una
  operación de editor, `providers` es una constante y `refs` se llama una vez por lanzamiento.
- **`fossil-lsp` y `fossil-wasm/src/lsp_worker.rs` son el mismo servidor dos veces** — ~600 líneas
  cada uno, doce handlers, cuatro derivas probadas bajo un docblock que se declara «the 1:1 model».
  Uno de los dos deja de existir, o la afirmación sale del comentario.

### El orden

1. **Los defectos vivos.** No es reorganización, pero ensucian cualquier medición posterior:
   `registry_key` a mano, `@rename` ignorado por cuatro de cinco, el `RunStatus` que anuncia un
   fichero borrado, `quantize` en f32 contra f64, la llamada viva a `classification()`, el módulo de
   direccionamiento inimportable, y `apps/corpus` sin correr en CI.
2. **Limpiar `fossil-base` y vaciar `fossil-engine`.** Son los dos que hacen todo el ruido, y hasta
   que no se muevan, nada del lado corpus puede salir.
3. **Sacar el corpus.** `graph_exec` y `layout` fuera de `fossil-runtime`; `fossil-mcp` con ellos.
4. **La API de referencia**, encima del direccionamiento que ya existe, con el corpus de conformidad
   como contrato y una segunda implementación que no comparta código.
5. **Las superficies de host**: disolver `fossil-run-status`, colapsar el LSP duplicado.

### Lo que NO se hace, y consta para que no se cuele

`core/ extensions/ corpus/` **no sale de las aristas.** Cada candidato a `core/` ya está separado por
algo real, y `fossil-base` —el único cuyo *contenido* está mal— quedaría archivado en vez de
arreglado. `extensions/` tampoco: `fossil-shex` está fuera por un ciclo, y una carpeta no disuelve un
ciclo. La única agrupación real que existe hoy es *compila a wasm32 o no*, y ya está enforced por
siete `compile_error!`.

### Y la conclusión que atraviesa las tres auditorías

**La prosa de este árbol carga peso y no la comprueba nada.** Nueve comentarios `///` afirman
invariantes que ningún test sostiene, y **cuatro de ellos causaron los defectos de la etapa 0**. Más
una regla dura invertida en `CLAUDE.md` («no `tokio` fuera de `fossil-lsp`», cuando `fossil-lsp` no
tiene `tokio`). El remedio está probado y es barato: **derivar el guard del original en vez de
repetirlo**, como hace `packages/introspect/tests/rust-parity.test.ts`, que lee el Rust como texto,
saca de él sus tres tablas y falla de seis maneras. Cada vez que abajo se borre un `///` que afirma un
invariante, lo sustituye un test.

### `.planning/` — el tercer plan, y por qué la mayoría ya no dice nada

Catorce documentos, 2.332 líneas, gitignorados. Auditados el 2026-08-14 **contra el árbol**, no
contra su propia cabecera. Casi todos describen cosas que ya pasaron o que se decidieron de otra
manera:

| documento | destino | por qué, comprobado |
|---|---|---|
| `RELEASE-RUNBOOK.md` | **→ `CONTRIBUTING.md`** | **Vivo y exacto**: `fossil-image.yml`, el `Dockerfile`, `fossil-mcp` y `release.yml` existen, y sus tres acciones de operador siguen sin darse |
| `HOST-BOUNDARY-MODEL.md` | borrar | `/docs/design/three-hosts` lleva la tesis, con un criterio más afilado que el original |
| `CONNECTION-SHAPE-MODEL.md` | borrar | su paso 3 lo superó `HOST-BOUNDARY-MODEL`, que a su vez ya está en el sitio |
| `OUTPUT-MODEL.md` | borrar | ShEx + SHACL → `GraphSchema` **se construyó**: `fossil-descriptors-output/src/shacl.rs`, y `catalogue` corre con una fila SHACL real |
| `W2-DUCKEXECUTOR-PLAN.md` | borrar | planifica **14 verbos**; el sitio del corpus documenta **seis**, y la cámara salió del enum (se direcciona, no se consulta) |
| `W0-MIGRATION-MAP.md` | borrar | su propio sucesor lo llama *stale* en la línea 3 |
| `PROJECT.md` | borrar | describe el DSL «con pipeline `\|>`», que es una tumba en `grammar.bnf` |
| `MILESTONES.md`, `ROADMAP.md`, `STATE.md`, `RETROSPECTIVE.md` | borrar | contabilidad GSD de hitos cerrados en mayo; `STATE.md` dice `status: completed`, `last_updated: 2026-05-28` |

**Tres se quedan, y no por cautela**: `EDITOR-SCHEMA-AWARE-PLAN.md`, `W0-SUBPROCESS-PLAN.md` y
`W3-LAYOUT-PLAN.md` describen la costura con **keasy**, que es otro checkout. Si están vivos, lo
están allí, y eso no se decide desde aquí. Lo que sí consta: los paquetes de editor se retiraron
enteros, y el fichero que `W3-LAYOUT-PLAN` nombra como ancla —`fossil-sinks/src/writer.rs`— **ya no
existe**.

### `phases/`, `milestones/`, `research/` — auditados el 2026-08-14

106.466 líneas en 332 ficheros no se auditan leyéndolas: se auditan por estructura. Dieciocho fases,
todas de v0.1 (cerrada el 26 de mayo) y v0.2 (cerrada el 28). Los `PLAN`/`SUMMARY`/`VERIFICATION` son
el registro de ejecución de dos hitos cerrados, y ese registro ya está en la historia de git como
commits reales. **Lo único que podía seguir vivo son los doce `deferred-items.md`** — lo que
explícitamente no se hizo — y cada item es una afirmación comprobable contra el árbol.

**De ~40 items diferidos, sobreviven tres.** Todo lo demás lo cerró lo que se construyó:

- El mayor de la fase 2 era **exactamente el hueco que se cerró esta mañana**: `lower_expr` no
  manejaba el prefixed-name y el test lo esquivaba fabricando un `HirExpr::PrefixedName` a mano.
  Llevaba tres meses anotado como «el arreglo es pequeño: resuelve vía la tabla de prefijos». Esa
  tabla ya no existe, y `08d67fe` borró la variante y el test.
- **LEXER-DIAG-01**, arrastrado de la fase 7 a la 8: el lexer se tragaba los bytes inlexables y
  `parse("#")` daba cero diagnósticos. **Cerrado**: hoy da exactamente uno que nombra el byte, con
  test en `fossil-syntax/tests/recovery.rs`.
- Todo el `cargo deny` de las fases 1, 2, 4 y 5 (wildcards, licencias, advisories): **verde**.
- Las derivas de `rustfmt` y `clippy` de las fases 1, 2, 4 y 9: **verdes** desde `a0e2700` y `5a5ece0`.
- Las fases **8, 10, 15, 16 y 17** difieren cosas del playground, el editor, el viewer y el puente
  CSS de keasy. Los paquetes se retiraron enteros: muertas por construcción.

**Los tres vivos, y están arriba en «deuda cosechada»:** el pin de `@duckdb/duckdb-wasm` en `latest`
(una pre-release de dev, `tests/wasm_parity/package.json`), `arrow`/`parquet` clavados en **58**
esperando el 59, y **enviar el correo a Labra Gayo**, que es una acción humana y no vive en ningún
otro sitio.

`milestones/` **borrado**: nadie lo citaba, y el propio audit de v0.2 dice de sus huecos *«ALL gaps
are bookkeeping or operational, not implementation»* — el operacional era REL-01, que ahora está en
`CONTRIBUTING.md`.

### Y por qué `phases/` y `research/` NO se borran todavía

**El código los cita 88 veces.** `RESEARCH.md` aparece en 28 ficheros fuente de ocho crates;
`P-CRIT-4` doce veces; los `Pitfall N` otras quince. Borrarlos ahora no limpia una referencia
paralela: **fabrica 88 citas muertas**, que es literalmente el defecto que el paso 9 existe para
cerrar y el mismo que dejó 159 referencias podridas con CI en verde.

Así que esto no es «pendiente de leer»: es **trabajo del paso 9, y lo agranda**. La operación es la
misma que con `decisions/` — cada cita se sustituye por lo que decía, o su contenido se muda a una
página — y sólo después se borra la carpeta. Los sitios están acotados: `fossil-syntax` y
`fossil-hir` concentran 18 de las 28.

### Lo que cada fase pendiente pide, cosechado del documento que se borró

**F3 · la caché de descriptores.** Va pronto porque es donde está el trabajo caro: programas
pequeños, fuentes grandes. (1) clave por **URI**, no por nombre de binding; (2) `content_hash`
poblado — mtime, ETag o hash, y el documento elige y dice por qué; (3) `register_inferred_descriptor`
deja de ser un `panic!` por defecto; (4) el registro como `Providers` de rustc — **hecho**, es el
registro de proveedores. **Hecho cuando:** cambiar el CSV y re-ejecutar re-introspecciona; no
cambiarlo, no.

**F4 · la forma es el contrato.** ~~Bloqueada por media hora de investigación: si la comprobación de
ShEx a medias decide que esto es SHACL.~~ **Caducado, comprobado el 14. No hay tal elección, y el
árbol la cerró hace tiempo.** SHACL es **una fila de proveedor más**, no una alternativa a ShEx:
`io.shex("x.ttl")` y `io.shacl("x.ttl")` conviven (`fossil-hir/src/stdlib.rs:442`) y
`fossil-descriptors-output/src/shacl.rs` baja el grafo de formas a `OutputShapes` —**la misma costura**
donde aterriza ShEx—, con `OutputShapes::to_graph_schema` dando al ejecutor una salida byte a byte
idéntica. La decisión está grabada en un renombrado: la variante `Shacl` es hoy `Lowered`
(`fossil-descriptors-output/src/kind.rs:40-50`), porque el nombre *«era una afirmación sobre el
idioma del documento que el valor no lleva»* —el registro de proveedores hace que un documento ShEx
llegue también ahí—, y lo que la variante significa es que **la decodificación ya ocurrió**. Por
debajo de esa línea el idioma es un detalle del proveedor.

~~El bloqueo real es fontanería: `lib.rs:470` devuelve `ACCEPT_ALL_DEFAULT`.~~ **También caducado, y
estaba mal atribuido.** Esa línea es cierta y pertenece al camino de **`run`**, no al del typecheck.
Son dos consumidores independientes del mismo documento:

- **run** → `resolve_output_descriptor` (`lib.rs:427`) → `read_output_shape` (`lib.rs:507`) →
  `Lowered(shapes.to_graph_schema())` (`lib.rs:546`), **uno por programa**, para el ejecutor.
- **typecheck** → `typecheck_mapping` (`check.rs:117`) → `resolve_target_shape`
  (`fossil-hir/src/shapes.rs:507`) → `decoded_document`, **uno por mapping**, resuelto por tipo
  (`def_map.rs:373`), y no pasa jamás por `resolve_output_descriptor`.

**El criterio de «hecho» está verde, medido el 15**: `cargo test -p fossil-engine --lib documents`
da 4/4, y `editing_the_document_rechecks_the_program_and_the_diagnostic_changes` mete una forma que
declara `xsd:integer` contra una columna `String` por la ruta de producción y asserta
``expected `Integer` ``. Y (1) y la mitad de (2) están hechos: propiedad no declarada
(`check.rs:827`) y primitiva incompatible (`check.rs:436`).

**Las dos trampas que este documento arrastraba están arregladas las dos** — y una de ellas dejó su
propio registro convertido en la cita falsa: `fossil-graph-schema/src/shapes.rs:155` afirmaba en un
docblock `pub` que el checker lee `value_ty: None` como `TyKind::Iri`, cuando `check.rs:455` hace
`expected.is_none_or(|e| subtypes(db, actual, e))` — ausencia acepta cualquier cosa. Corregido.

**(3) nombra una función que no existe.** `anon.hmac` está borrado del lenguaje (`stdlib.rs:894`:
DuckDB tiene `sha256` y no tiene HMAC), y `book/stdlib.mdx` lo documentaba. Si la sensibilidad como
tipo sigue queriéndose, necesita primero una primitiva que la produzca.

**Lo que queda, y ninguno bloquea:** ~~borrar las cinco variantes muertas de `TargetShapeError`~~
**hecho el 19** —cuatro borradas con sus brazos, 85 líneas, y la sonda de alcanzabilidad convertida
en el test en vez de en un comentario; falta sólo `NoDocument`, bloqueada por tres líneas de
`fossil-ide`—; dar span de dos ficheros al desajuste, que es lo que los cinco
`expected/diagnostic.txt` escriben a mano hoy; y **la cota superior de cardinalidad, honestamente
bloqueada**, porque nada en el lenguaje construye un `Seq` y un `maxCount 1` no tiene qué rechazar.

**El agujero del join NO es de F4**, y esto se decidió mirándolo: vive en el lado *fuente*, aguas
arriba de cualquier forma, y salta en un programa que no nombra documento (`resolve_source_scope`
corre en `check.rs:104`, antes de `resolve_target_shape` en el `:117`). `apply_source_op`'s `Join`
(`infer.rs:589`) llama a `check_refs`, que sólo comprueba que cada referencia resuelva: **ningún tipo
se lee**. Es su propio cambio, ~15 líneas, y es el gemelo tipado y temprano de la comprobación que
`fossil-df/src/plan.rs:190` ya hace estructuralmente.

**F5 · el pipeline.** Hay una decisión antes de empezar: **si `join` entra en la primera versión**,
que es lo que hace difícil el checker; `where` y `select` puede que basten un tiempo. MIR tipa,
DataFusion planifica.

**F6 · Salsa se encoge.** Medido ya: el camino por lotes compra tres entradas clavadas por fichero
—`parse`, `def_map`, `lower_to_hir`— y las cinco por mapping ejecutan una vez cada una **sin un solo
acierto**. (1) el camino por lotes deja de construir un `FossilDb`: tres `OnceCell` en un contexto;
(2) `fossil-df-wasm` deja de construir una base de usar y tirar para alojar una consulta; (3) Salsa
se queda en los crates que sirven al editor. **Hecho cuando:** `grep salsa crates/*/Cargo.toml` no
encuentra `fossil-engine` ni `fossil-df-wasm`, `fossil check` da lo mismo, **y se cronometra antes y
después** — la medición cuenta ejecuciones, no milisegundos, así que el −52 % de Apollo es una
estimación prestada hasta que sea nuestra.

**F7 · el escritor de Parquet. Medida el 15, y la medición la revierte.** El arnés vive en
`spikes/f7-writer-bench/` (`./run.sh`, más `--inspect` sobre un Parquet ya escrito), fuera del
workspace, y `crates/fossil-df/examples/tile_layout.rs` reproduce cada cifra por su cuenta: dos
arneses, los mismos números. 5M filas, las mismas filas en el mismo orden por los dos caminos.

**El titular: `arrow-rs` no compensa. Lo que compensa es el tamaño de row group, y DuckDB lo da con
una palabra de SQL.**

- ~~(1) `arrow-rs` en vez de `COPY`, porque escribe el índice de páginas y DuckDB no.~~ **Falsa por
  la razón, no por el hecho.** El índice se escribe (179.419 B) y **salta 0 páginas y 0 bytes**: a
  4.096 filas por row group hay exactamente **1 página de datos por column chunk**, así que el
  min/max por página duplica byte a byte la estadística del chunk que el footer ya lleva. Podar por
  página cuesta 156,4 peticiones donde podar por row group cuesta 5,6, por los mismos bytes (1,21
  MB). Y DuckDB escribe 1 página por chunk **incluso con su default de 122.880 filas**, luego el
  índice tampoco valdría nada en un fichero suyo. Coste: **1,263×** bytes con los defaults que la
  propuesta decía no tocar, **2,861×** en el esquema ancho; la paridad (1,015×) exige apagar el
  diccionario por columna, que es justo la política explícita que la claim daba por innecesaria.
- **(2) un fichero con row groups de 4.096, no un fichero por tesela: se sostiene, y es la victoria
  entera.** 22,3 → **5,6** peticiones por ventana, y el índice que un lector debe adquirir pasa de
  494,9 kB en **1.221 peticiones** a 330,1 kB en **una**. Pero es un cambio de *disposición*, no de
  escritor: `COPY (FORMAT PARQUET, ROW_GROUP_SIZE 4096)` le gana a los defaults de `arrow-rs` en
  todas las columnas (1,09 MB contra 1,38 MB por ventana; 330 kB contra 676 kB de metadatos).
- **(3) el footer como directorio raíz y las cajas `x`/`y` como índice: se sostiene.** El 1,05×–1,21×
  reproduce como **1,10×–1,20×, media 1,14×**, y es propiedad del teselado Morton, no del escritor.
- **La estimación de 566 kB falla por los dos lados**: el footer observado son 496.373 B (−12 %), y
  footer + índice de páginas 675.792 B (+19 %).

**Qué revertiría esta reversión:** una tesela más grande —a 32.768 o 122.880 el índice empieza a
saltar (medido: 97,6 % de las páginas, 41,70 MB → 2,98 MB)—, pero el 4.096 lo fijó la medición previa
de peticiones por ventana, así que reabrir uno reabre el otro; una página de datos más pequeña que la
tesela; un destino **sin proceso DuckDB** —el camino del navegador ya escribe con `arrow-rs`, y ahí
el escritor no se elige y sólo queda la pregunta de bytes: diccionario apagado + snappy, 1,015×—; o
que DuckDB empiece a escribir el índice, que mataría la claim 1 por el otro lado.

**Las aristas, medidas también el 15 (20M y 40M, grados 4 y 8). No cambian la reversión: la
ensanchan** — pero traen el único argumento medido a favor de `arrow-rs` que existe en los dos
informes, y un agujero del escritor que no sabíamos que teníamos.

- **La claim 1 muere en las dos formas.** El caso que podía resucitarla se dio —a grado 8 la tesela
  cruza el límite de página de arrow y hay **2,00 páginas por chunk**, no 1,00— y aun así un lector
  guiado por `ColumnIndex` salta **cero páginas y cero bytes**, byte a byte idéntico. La razón es
  estructural: **un row group ES una tesela de origen, y la clave de poda ES la tesela de origen**,
  así que el índice y la clave son la misma clave. Partir un chunk en dos páginas lo parte en dos
  páginas que están las dos seleccionadas.
- **Las aristas son la mayoría de la carga**: 64 % de los bytes a grado 4, 77 % a grado 8, y **la
  mitad exacta de las peticiones** con el esquema de un fichero por tesela. El total por ventana pasa
  de **44,7 peticiones a 9,2**. Y la etapa 2 no necesita índice ninguno: **la tesela de aristas `k`
  es la tesela de vértices `k`**, así que la caja de vértices selecciona las URLs por identidad.
- **El único argumento medido a favor de `arrow-rs`, y es real:** las teselas de aristas tienen
  cuentas de filas **variables** (mediana 16.384, mín 11.452, máx 16.731 a grado 4), y
  `ROW_GROUP_SIZE` es un número fijo, así que los row groups de DuckDB **cruzan fronteras de tesela**:
  55,8 páginas por ventana contra 44,7, y 2,41 MB contra 1,93 MB. Row groups alineados a tesela son
  ~20 % menos bytes por ventana y **sólo `ArrowWriter::flush()` sabe cortarlos ahí**. Si esa forma se
  adopta como objetivo, la claim 2 se vuelve un argumento por el escritor después de todo.
- **Y los bytes no generalizan**: `arrow-rs` con defaults es 1,176× a grado 4 pero **0,939× a grado
  8**. El «26 % más gordo» del lado vértice era de columnas anchas; en enteros estrechos con filas
  suficientes por grupo, el diccionario gana.

~~**El agujero: CSC no está teselado en absoluto.**~~ ~~**Y la deuda del manifiesto queda
confirmada.**~~ **Las dos CADUCARON entre el 16 y el 19, y las dos las cerró otra sesión mientras se
medían.** Consta aquí en vez de borrarse porque el modo de fallo es el que este plan persigue: una
medición correcta el día que se tomó, escrita en presente, y falsa tres días después sin que nada
avisara. Comprobado contra el árbol el 19:

- **`by_target` SÍ está teselado.** `fossil-runtime/src/layout.rs:690-745` recorre **todas** las
  adyacencias por `ordered_by` y escribe `by_target/tile{k}.parquet` cortado por `dst_dense` con el
  desplazamiento del tipo destino. `fossil-engine/tests/conformance.rs:379-410` afirma las dos
  orientaciones, y los guards de `apps/corpus` comprueban las dos.
- **El manifiesto SÍ se autodescribe.** `fossil-sinks/src/manifest.rs:163-188` — `AdjList` lleva
  `prefix` (`by_source/`, `by_target/`), afirmado en `edge_yaml_carries_graphar_v1_field_names`. Un
  lector tercero **puede** derivar la URL de una tesela de aristas:
  `<prefix de arista><prefix de adj>tile{dense >> shift(src|dst_chunk_size)}.parquet`.
- **Lo que sí queda del manifiesto** es otra cosa y más pequeña: **ningún manifiesto lleva el número
  de vértices**, así que `tile_count = ceil(V / chunk_size)` —que la página de lectura afirmaba— no
  se puede calcular. La respuesta honesta hoy es la sonda `HEAD` doblando y bisecando.

Lo que **no** ha caducado son las cifras: 19.892 aristas por ventana a grado 4 y 39.849 a grado 8
tienen destino dibujado y origen fuera de la ventana. Eso ya no es un hueco del escritor —hay tesela
que pedir— sino el argumento de por qué **una capa incompleta tiene que declararse incompleta**, que
es lo que `Window.complete` y `gaps[]` hacen ahora en `@fossil-lang/graph`.

**No medido, y consta:** la distribución de grados es estipulada (~uniforme) y una real es de cola
pesada, lo que mueve filas por tesela → páginas por chunk → el margen de la claim 1; qué costaría una
consulta de in-edges por range-request sobre ese fichero de 105 MB; una distribución de comunidades
sesgada como la que daría Leiden; y si la puerta WASM pasa con arrow 59.

**Y `arrow`/`parquet` 59 ya está publicado** (59.2.0, y la línea 58 ganó un 58.4.0). El arnés compiló
contra 59.2.0 **sin un solo cambio de fuente** y su informe salió **byte a byte idéntico**: nada de
lo medido se mueve al subir el pin. El pin del repo no se ha tocado.

**Las slices de layout, cosechadas de `W3-LAYOUT-PLAN.md`** — que resultó no ser de keasy:
`fossil-runtime/src/layout.rs` lo citaba. La primera está construida (comunidades por modularidad,
la jerarquía, el morton-sort); las cuatro siguientes añaden **una** capacidad pesada cada una detrás
de la costura que la primera dejó estable, y cada una necesita su decisión antes de empezar:

| slice | trabajo | la decisión que necesita |
|---|---|---|
| **W3.2 Leiden** | dep `single-clustering`; Leiden por componente WCC para partir la componente gigante; `cluster_id` plano = (componente, sub-Leiden) | auditoría de dependencia (rayon, encaje de la licencia BSD-3); si se guardan los niveles de la jerarquía, que pide extender el manifiesto; el parámetro de resolución por defecto |
| **W3.3 ForceAtlas2** | dep `forceatlas2`; sembrar desde las posiciones de la slice 1; iterar a convergencia; re-morton-sort | iteraciones contra presupuesto de reloj a 1–5M nodos; theta de Barnes-Hut; **y la de verdad**: si se topa N para la pasada en RAM y por encima se cae a las posiciones sembradas — ése es el techo real de larger-than-RAM |
| **W3.4 Embeddings** | columna opcional de embedding por nodo para búsqueda semántica | modelo y dimensionalidad; si los embeddings dirigen el layout (estilo UMAP) en vez de FA2; la historia fuera de memoria |
| **W3.5 Resúmenes de cluster** | `cluster_summary` generado por LLM para `summarize_cluster` | dónde viven los resúmenes (Parquet aparte o manifiesto); qué superficie LLM; granularidad de nivel. **Es una decisión de producto, no de layout** |

**Track paralelo · RDF 1.2.** No es una fase, es una obligación que atraviesa F2 y F4. Lo que se
puede hacer ya: la dirección base en literales (`rdf:dirLangString`, `@en--ltr`) — el perfil
`1.2-basic` es exactamente el productor que la emite sin reificación y es el primer objetivo
realista; y encender la feature `rdf-12` de `oxttl`, cuya medición está en el `Cargo.toml` raíz. El
triple term como primitiva necesita F2; el almacenamiento con discriminador necesita F7.

### Lo que sobrevive del traspaso de kanzo-ui, mudado aquí el 19

`HANDOFF-FROM-KANZO-UI.md` pedía cinco cosas y se borra con este commit, porque su primera línea lo
exige y porque `CLAUDE.md` no admite una segunda referencia. Sus peticiones 1, 2 y 3 aterrizaron —el
direccionamiento salió a `@fossil-lang/graph`, hay corpus de conformidad en `apps/corpus`, y el
contrato de completitud es `Window.complete` + `gaps[]`. Lo que queda:

- **No hay forma duradera de nombrar un vértice.** Rehacer el layout renumera, no leemos `subject`, y
  el corpus no publica versión que un cliente pueda comparar. **Es nuestro y no tiene sitio todavía**
  — un cliente que guarde una selección no puede volver a ella tras un relayout.
- **Ningún manifiesto lleva el número de vértices**, así que `tile_count = ceil(V / chunk_size)` no se
  puede calcular y la respuesta honesta es sondear con `HEAD` doblando y bisecando. Escrito como
  pregunta de diseño en `reading/without-fossil.mdx`, no encodado.
- **Avisar a kanzo-ui cuando aterrice la disposición de un fichero con row groups de 4.096**: su
  lector deja de sondear footers y la caché de cajas que tenían pensada se vuelve innecesaria.
- **Suyos, anotados para no volver a medirlos:** el `fetch` global y no inyectable en su lector —sin
  auth, sin OPFS, sin doble de test—, y un tipo de vértice por corpus abierto cuando el KG tiene
  varios, sin estar escrito como límite.
- **Devuelto medido y no es de nadie de aquí:** el suelo de zoom. Cinco millones no se encuadran —
  cosmos.gl corta en 1e−3 y hacen falta 1,51e−4, **6,61× corto**. Y el 48,9 % de las aristas cruzan
  `cluster_id`, la mitad atravesando el lienzo entero: eso es del paso de layout, no de quien dibuja.
- **Deuda de honestidad, nuestra:** el 1,87× de `subject` está citado más fuerte de lo que la
  medición sostiene, en tres sitios.

---

**Deuda cosechada que no bloquea a nadie**, y que **no he verificado hoy**: que `fossil-engine`
rechace destinos cloud pese a que el emisor está escrito para URLs, y que un lector tercero no pueda
derivar del manifiesto la URL de una tesela de aristas (caen en
`edge/<dir>/by_source/tile{k}.parquet` y el YAML sólo declara `prefix: edge/<dir>/`). Las otras tres
de aquella lista están cerradas: `cargo fmt` (`a0e2700`), `timeout_ms` (`329d7ef`), y los quads, que
`apps/corpus/content/docs/conventions/adjacency.mdx` cubre.

**Deuda cosechada de `.planning/phases/`, verificada hoy y viva:**

- **`@duckdb/duckdb-wasm` pinado a `latest`** en `tests/wasm_parity/package.json` — es una
  pre-release de dev (`1.33.1-devN.0`); el último estable es 1.32.0. Repinar a un 1.33.x final
  cuando salga. El arnés de paridad es lo que detecta una divergencia nativo↔WASM, así que un pin
  móvil es un arnés que puede cambiar bajo los pies.
- **`arrow` y `parquet` clavados en 58**, porque la línea 59 no estaba publicada cuando se pinó.
  **Ya lo está** (59.2.0, comprobado el 15), y **el pin dejó de estar acoplado a F7**: la medición
  revirtió el cambio de escritor, así que ya no hay ningún «no se cambia el escritor sobre un pin
  viejo» que esperar. Sube cuando quiera, y lo único que hay que volver a pasar es la puerta WASM —
  que es la parte **no medida** de aquel informe.
- **Enviar el correo a Labra Gayo.** Es una acción humana, arrastrada desde la fase 0, y no vive en
  ningún otro sitio del árbol.

**Deuda del propio documento, antes de F8:** el diagrama de `/docs/architecture` nombra `registry` en
el grupo *language* y ese crate se borró en `510eb87`, y **omite `fossil-lineage`**, que es parse-only
y WASM-clean y pertenece a ese grupo. Los totales cuadran en 24 por casualidad: uno compensa al otro.
El recuento de aristas («43») es de antes del borrado y no se ha recalculado. `content.test.ts` no
puede verlo, porque comprueba rutas citadas y un rótulo de nodo mermaid no es una ruta.

---

## Lo decidido el 2026-08-13, y gobierna

9. **La promesa de `grammar.bnf` se cumple literalmente: el conjunto de conformidad crece a 23.**
   Cinco programas nuevos deletrean el ternario, `or`, el aditivo, el multiplicativo y el unario. Es
   la única opción que además **prueba** que esas cinco producciones funcionan — hoy nadie lo sabe,
   porque ningún programa las ejercita. Un comentario habría cerrado la promesa dejando el agujero.
10. **fossil lee ShEx y SHACL, y el nombre del constructor pasa a ser PORTANTE.** Hoy es decorativo:
    `def_map.rs:319` hace `let (_ctor, document) = parse_source_call(&item)` y **tira el nombre**;
    todo el despacho va por la extensión del fichero (`decoder_for`). Así que `io.shex("x.ttl")` y
    `io.shacl("x.ttl")` hacen hoy exactamente lo mismo — dos sitios dicen la misma cosa y sólo se lee
    uno, que es cómo discrepan en silencio. A partir de ahora **el nombre selecciona el decodificador**
    y una extensión que no le corresponde es un **error que nombra los dos**. La redundancia deja de
    ser una segunda grafía y pasa a ser una concordancia comprobada.

    **Y el precio no es el que parecía: añadir LinkML NO toca la gramática.** `io.linkml("x.yaml")`
    es `PostfixExpr := PrimaryExpr (DOT IDENT | call)*` — la gramática ya deriva cualquier
    `io.<ident>(…)`, y lo que resuelve el nombre es el catálogo. Es ADR-0059 §3 tal cual: una
    capacidad nueva es una **fila**, nunca una regla. Lo único que se paga es que el autor diga dos
    veces lo mismo, y a cambio las dos afirmaciones se comprueban.

    **La raíz del hack, y es lo que de verdad hay que arreglar: hay DOS tablas para una idea, y
    despachan por criterios distintos.**

    ```
    stdlib.rs            SourceKind   { short_name: "csv", extensions: ["csv"],                 lowering }
    stdlib.rs            SourceKind   { short_name: "rdf", extensions: ["ttl","nt","n3","rdf"], lowering }
    shape_documents.rs:71  ShapeDecoder { name: "shex", extensions: ["shex","shexj","shexc"],   decode }
    ```

    Los mismos tres campos —un nombre, las extensiones que acepta, y qué hace con ellas— modelados dos
    veces. La de datos despacha por nombre; la de esquemas por extensión, y por eso el nombre del
    documento acabó siendo decorativo. El propio doc-comment de `ShapeDecoder` dice que `name` está ahí
    «para un host que quiera seleccionar una fila **por nombre**»: la capacidad está escrita y sin usar.

13. **UN registro. La fila declara qué sabe hacer; la posición sintáctica elige cuál se le pide.**

    Las dos tablas se colapsan en una. Una fila es: **el nombre que se escribe tras `io.`**, las
    extensiones que acepta, y **sus capacidades**. Despacho **siempre por nombre**; la fila comprueba
    su propia extensión y emite su propio rechazo, con sus palabras.

    **La dirección no es un eje, son dos** — por dónde van los bytes, y qué describe la cosa:

    | | describe **datos** | describe **tipos** |
    |---|---|---|
    | **lee** | `io.csv`, `io.parquet`, `io.rdf` | `io.shex`, `io.shacl` |
    | **escribe** | sin sintaxis (hoy, la bandera `--dest`) | nada — y sin embargo ya ocurre |

    Dos cosas que sólo se ven con la tabla delante. **`io.shex` es de entrada por los bytes y de salida
    por el significado**: se lee un fichero y lo que describe es el contrato de salida, que es
    literalmente lo que hace `output_shape_document` — por eso nunca encajaba en un eje de un solo
    sentido. Y **la celda de abajo a la derecha ya existe en el artefacto y no en el lenguaje**: el
    manifiesto de GraphAr es un documento de tipos escrito, y el lenguaje no tiene cómo nombrarlo.

    Un formato **no tiene dirección** —Parquet es Parquet se lea o se escriba, y un corpus GraphAr lo
    escribe `fossil-runtime` y lo lee `fossil-graph`—, así que partir `io` en dos namespaces habría
    sido dos nombres para una cosa. La fila declara; la posición elige:

    ```
    User := io.csv("users.csv")          binding → lee filas
    type { P } := io.shex("shop.shex")   tipo    → lee tipos
    type { P } := io.csv("users.csv")    ERROR: `io.csv` lee filas, no tipos
    ```

    **Pedir a una fila una capacidad que no declara es un error que nombra las dos.** Y lo único que
    una fila **no** posee es la comprobación hacia atrás, que necesita el vocabulario común entre
    lenguajes: la fila traduce a él, y lo que su lenguaje exprese y el vocabulario no recoja lo
    diagnostica la fila.

    **Lo que esto NO decide: la sintaxis de destino.** `grammar.bnf` avisa de que inventarla antes de
    decidirla es como v0.1 se llenó de fantasmas. La tabla deja el hueco preparado y nada más.

    **Aterrizado el 2026-08-13**, en `crates/fossil-base/src/providers.rs`. Tres cosas que sólo se
    vieron al construirlo:

    - **La identidad de fila era un bug latente.** Era `ptr::eq` sobre la dirección de `decode` — y una
      fila que lee filas **no tiene `decode`**, así que **las cuatro filas de datos habrían comparado
      iguales**. Ahora la identidad es la dirección de la fila, y `Provider` **no es `Copy` ni
      `Clone`** a propósito, para que una copia no pueda convertirse en silencio en otra fila.
    - **`.ttl` lo reclaman ahora DOS filas con dos capacidades**: `io.rdf` lee filas de un grafo,
      `io.shacl` lee tipos de un grafo de formas. Eso era **inexpresable** bajo despacho por extensión,
      y es la segunda mitad de por qué las tablas tenían que fundirse.
    - **`catalogue` no fallaba por falta de decodificador, sino por producir el modelo equivocado**: el
      recorrido SHACL producía `GraphSchema` —el modelo del ejecutor— y nunca `OutputShapes`, que es el
      vocabulario que lee el checker. Moverlo arregló además que el recorrido era **alfabético por IRI
      de sujeto**, lo que **cambiaba en silencio qué liga un `type { A, B }` posicional**.

    **El hueco de `schema =`, cerrado el 2026-08-13: nombra un proveedor.**
    `{A,B} := io.rdf("g.ttl", schema = io.shex("x.shex"))`. Era la última selección por extensión del
    árbol, y con esto queda **una sola regla en todo el lenguaje: donde hay un documento, hay una fila
    que lo nombra.** Se paga una línea algo más larga.

    Queda un tercer criterio de despacho, y no lo cubre este dictamen porque el host no pasa nombre
    ninguno: `fossil-df-wasm::build_descriptor` **olfatea el contenido** (`text.contains("sh:NodeShape")`).
    Recibe un blob opaco, así que no hay nada que nombrar; si esa costura llega a llevar nombre algún
    día, muere también.

    Y aparte de la decisión: `read_output_shape` (`fossil-engine/src/lib.rs:427`) deja de ser
    sólo-ShExJ. Que un documento type-checkee y luego haga fallar el `run` es un fallo en cualquier
    lectura.

    **Qué es `catalogue`, ya que su razón de ser no estaba escrita en ningún sitio:** es el único de
    los programas de conformidad cuyo documento de tipos **no es ShEx**, y existe para probar que la
    costura del documento de forma **no es específica de ShEx**. Es un test de costura disfrazado de
    tienda, y por eso es también el único que falla por no haber decodificador de SHACL.
11. **El constructor de aristas liga POSICIONALMENTE, y cada argumento es el valor terminado del
    hueco.** La enésima expresión llena el enésimo hueco de la plantilla del tipo destino, en orden de
    aparición. Es la misma regla de ligadura que ya usa `type { … }`, así que el lenguaje tiene **una**
    y no dos. Ligar por nombre habría convertido el nombre del hueco en API pública del tipo.
12. **El documento de forma es el segundo fichero.** Ir a la definición sobre un nombre de forma en
    una cabecera salta al `.shex`; sobre una clave de propiedad, al predicado que la declara. El
    escenario de dos ficheros sobrevive con más sentido que antes: cruza una frontera de **lenguaje**,
    no de prefijo.

    **Implementado el 2026-08-13, y `canonical_200_b.fossil` se BORRÓ en vez de reorientarse.** El
    argumento: no hay producción de import y un fichero se compila solo, así que **no existe ninguna
    posición IDENT de un programa fossil que pueda nombrar algo de otro `.fossil`** — los nombres de
    forma y las claves de propiedad van al `.shex`, los de fuente son locales, y los de mapeo son
    declaraciones. El segundo fichero **es** el `.shex`, y ya existe: `canonical_200.shex`.

    Detalle honesto de la implementación: el decodificador **no guarda offsets**, y no puede ganarlos
    barato — salsa memoiza `OutputShapes` y decide invalidación por `PartialEq`, así que un campo de
    span reexaminaría cada mapeo ante cualquier edición de espacios. La posición se recupera con un
    **localizador textual** sobre los bytes del documento, con guardia de frontera de nombre para que
    `…/name` no case dentro de `…/nameOfThing`. Si falla, devuelve el fichero correcto con `0..0`.

14. **El catálogo es declarativo, y pasa a ser parte de la baseline.** Una fila es **receptor + nombre
    + firma + lowering**. El receptor es un espacio (`io`, `clean`, `parse`, `math`…), un tipo (`str`,
    `seq`, `User`) o una relación — ADR-0059 §1 llevado hasta el final: el punto significa «miembro
    de», y **todo miembro es una fila**. La gramática se queda con literales, identificadores, acceso
    a miembro, llamada y las formas de ligadura; **todo lo demás es dato**.

    Con eso, `grammar.bnf` dice **la forma de los programas** y el catálogo dice **qué nombres
    existen**: dos ficheros de datos y un compilador. Añadir `str.slugify` sobre `regexp_replace` deja
    de tocar Rust.

    Hoy el catálogo tiene el mismo defecto que tenía la tabla de decodificadores: `RegistryEntry.name`
    es *«el nombre punteado completo»* — la cadena `"clean.trim"`. Despacha por cadena, que es
    exactamente lo que ADR-0059 §3 manda sustituir por despacho por tipo de receptor.

15. **`LoweringKind` colapsa de 13 variantes a DOS: `Expr(plantilla)` y `Op(operador)`.**

    El defecto medido: `LoweringKind` tiene 4 variantes e `InlineForm` otras 9, y **las nueve son
    plantillas SQL cuyo texto ya está escrito en sus propios doc-comments** — `CAST(x AS <t>)`,
    `a || b`, `split_part(s, sep, n)`, `json_extract(s, path)`, `CASE WHEN x IS NULL THEN error(…)`…
    El dato existe; está en un comentario, donde nada lo puede ejecutar. Y dos de ellas —`SplitPart`
    y `JsonExtract`— son **exactamente lo que es `Builtin`**: una llamada por nombre. La frontera
    `Builtin`/`Inline` no es semántica, es un accidente de quién escribió qué como variante.

    `Plan(PlanOp)` sí corta por un sitio real —nombra un operador del álgebra, un conjunto cerrado de
    14— pero se llamaba por su efecto en vez de por lo que nombra.

    **Y `Foreign` desaparece.** De las ocho filas con UDF nativa, **dos se borran del lenguaje** por no
    poder hacerse bien sin Rust —`anon.hmac` (HMAC necesita calendario de claves; DuckDB tiene
    `sha256` y no HMAC) y `clean.normalize_unicode` (DuckDB trae `nfc_normalize`, sólo NFC)— y las
    otras seis bajan a plantilla SQL.

    **Lo que se cae con ella no es cosmético:** `WasmClass` desaparece como *concepto*, no sólo como
    campo — con `derive_wasm_class`, la variante `NativeUdfOnly` y el test del invariante
    `PureSql ⟺ no-Udf`, cuyo estado malo pasa a ser irrepresentable en vez de derivado-y-comprobado.
    Y con ella `crates/fossil-runtime/src/udf.rs` entero. **El lenguaje pasa a correr entero en el
    navegador**, que es un cambio de producto y no de limpieza.

    **Lo que hay que medir y no suponer:** `validate.email` y `validate.url` **cambian de
    comportamiento** al pasar a regex — validar bien no es una regex. Hay que dar el delta con
    ejemplos en las dos direcciones, y si es inaceptable la salida es borrarlas también, no publicar
    una validación que miente.

16. **`clean` se funde en `str`.** `str/` tenía 8 operaciones sobre cadena (`length`, `slice`,
    `contains`, `starts_with`, `ends_with`, `replace`, `split`, `concat`) y `clean/` otras 5 (`trim`,
    `lower`, `upper`, `slug`, `strip_html`) — **la misma clase de cosa, sin ningún principio que las
    separe**. `replace` podría llamarse limpiar y `trim` podría llamarse operación de cadena: son dos
    espacios para una idea, que es lo que prohíbe la regla 2.

    Y eso explica el defecto que lo destapó: ADR-0059 §2 y `grammar.bnf:381-385` usan
    `str.lower(str.trim(x))` como **el** ejemplo canónico de «una entrada por dos caminos», y esas dos
    filas no existen. Quien escribió la ADR **escribió los nombres que un lector espera**. El ejemplo
    no estaba mal por descuido: acertaba en dónde deben vivir esas funciones, y el catálogo era lo
    equivocado. Cinco renombres y el documento de referencia pasa a ser cierto sin tocar una línea.

    `parse`, `validate`, `math` y `anon` **no se tocan**. Si tienen la misma contradicción, se decide
    otro día.

17. **El `join`: gana la forma predicado, y deja de aplanar.** Las dos contradicciones que quedaban se
    resuelven solas con lo ya decidido.

    - **La clave.** `lower_source_stage` y ADR-0054 §3 exigen `on = .k` (semántica `USING`), pero `.k`
      es un `FieldRef` y **`FieldRef` ya no existe**. Gana `on = Purchase.user_id == User.id`, que es
      lo que escriben `grammar.bnf` y el fixture de referencia.
    - **El aplanado.** Hoy `infer.rs::apply_source_op` calcula `fila(izq) ⊎ fila(der)` —un registro
      **plano**— y trata cualquier nombre compartido como **error** («rename one side before
      joining»). Bajo ADR-0059 el cuerpo escribe `Purchase.amount` y `User.email`: los dos lados
      siguen direccionables bajo su nombre de binding. Así que **la regla de colisión se borra, no se
      relaja** — la cualificación es exactamente lo que quita la ambigüedad que esa regla vigilaba.

    Es el único cambio que **altera el significado de un programa que hoy se acepta**: dos fuentes con
    una columna homónima pasan de rechazadas a legales.

## Lo que se borra del corpus viejo, decidido con evidencia

**`packages/examples/` entero — los 36.** No es que los 18 lo cubran: es que el paquete no tiene a
quién servir. Es `private: true`, el grep de `@fossil-lang/examples` fuera de sí mismo no devuelve
nada, y **el consumidor para el que se construyó, `packages/playground`, ya no existe**. Su arnés
comprueba códigos de salida y regexes de stderr, nunca la salida, y no hay un solo golden en disco.
Los 36 están en sintaxis vieja de punta a punta y ninguno se migra editando una línea. 24 serían
idénticos en intención a `hello`/`catalogue`/`shop`; 6 más aportan sólo el enlace por plantilla de
IRI, que los 18 redeletrean como constructor de aristas; y los 6 de `typing-showcase` existen **para**
`|>` y `filter(.age >= 18)`, los dos enterrados. `06-add-prefix` es directamente irrepresentable: no
hay prefijo que añadir. Se lleva por delante `.github/workflows/examples.yml` y el test de fijación
de `examples.test.ts`, que **clava la cadena `prefix ` como invariante** y se pone rojo solo en cuanto
`prefix` muera.

**`tests/fixtures/canonical_200*.fossil` se migran, y NO son conformidad: son un banco de pruebas.**
`canonical_200.fossil` (172 líneas, 8 fuentes, 15 mapeos) es el fixture de
`crates/fossil-lsp/tests/didchange_budget.rs`, **una puerta dura de CI a 400 ms** — un presupuesto que
no significa nada medido sobre un fichero de 12 líneas, y el mayor de los 18 es `shop` con ~30. Además
deletrea `clean.*`, `math.round` y `str.concat` de dos argumentos, espacios que `grammar.bnf` nombra y
ningún programa de conformidad ejercita. Dos condiciones portantes: `crates/fossil-lsp/tests/lsp_features.rs`
localiza los cursores por búsqueda literal de cadena y hay que editarlo **en el mismo commit** o revienta
con «needle not in fixture»; y `crates/fossil-lsp/benches/baseline.json` clava la ruta con un umbral de
regresión del 20 %. `canonical_200_b.fossil` es el **único escenario de dos ficheros del repo**, y su
razón de ser es la resolución de `prefix` entre ficheros — con nombres desnudos e IRIs completos en
cadenas **no queda ningún nombre entre ficheros que resolver**, así que qué debe referenciar el segundo
fichero es una pregunta de diseño abierta, no un port mecánico.

## La cabecera de `grammar.bnf` incumple su propia promesa cinco veces

Dice: *«no hay abajo ninguna producción para una forma que ninguno de los 18 deletree, salvo donde un
comentario lo diga y diga por qué»*. Cinco producciones no tienen programa **ni comentario**:
`TernaryExpr` (L1), `OrExpr` (L2, sólo se usa `and`), `AdditiveExpr` (L5), `MulExpr` (L6) y
`UnaryExpr` (L7 — y `not` es palabra **reservada**, que cuesta a todo programa que la quisiera como
nombre de columna). `MultiSourceDef` es el modelo a seguir: no se deletrea y lleva el comentario que
lo dice. O se debilita la promesa de la cabecera, o se deben cinco comentarios.

## El paso 9 es el final, y es un borrado

No es «limpiar»: es **quedarse con una sola referencia**. Hoy la verdad está repartida entre las
ADRs, cinco documentos de diseño de la raíz y dos webs, y esa dispersión es lo que produce las
citas muertas —159 medidas— y los 18 de 20 `ADR-0050` que resuelven al registro equivocado.

Lo que queda al final:

- **`grammar.bnf`** — el lenguaje, normativo. Su cabecera ya declara la inversión: el parser la
  implementa, no la define.
- **`apps/docs/programs/`** — los 18 programas, ejecutados. La gramática los nombra como su único
  control, y el paso 8 los convierte en test.
- **`apps/docs`** (el lenguaje) y **`apps/corpus`** (el grafo). Dos productos, y la costura es el
  corpus en disco.

Lo que desaparece:

- **`decisions/` entero.** Antes hay que **cosechar** lo que no vive en ningún otro sitio —las
  mediciones, el arte previo consultado, el campo «qué revertiría esto»— y hacer la cirugía de
  `apps/docs`, que hoy depende de la carpeta por dos caminos y pondría la build roja dos veces.
- **`architecture.md`, `type-system.md`, `operator-algebra.md`, `stdlib.md`**: absorbidos por las
  páginas nuevas, y ninguno es dependencia de build. `grammar.bnf` se queda.
- **Todo el registro de compatibilidad hacia atrás.** No hay nada publicado, no hay consumidores, y
  una página que explique de dónde venimos es una segunda referencia. La baseline no tiene historia:
  dice lo que el lenguaje es.

El campo `Status` de las ADRs no significa nada hoy —doce dicen `proposed` y siete están
construidos—, y eso no se arregla: se cierra borrándolas cuando su contenido esté en la baseline.

---

## El paso 8 cambia de naturaleza

La escalera vieja terminaba en «reescribir los 74 fixtures una vez». Ya no: **los programas de la
documentación SON el corpus de aceptación**. La prosa nunca contiene un programa, lo transcluye
desde `programs/`, y un test compila cada uno y guarda su salida **o su diagnóstico**. Un ejemplo
rancio deja de compilar en vez de pudrirse en silencio, y el texto de los errores pasa a ser
artefacto probado.

Eso sustituye a la comprobación de citas `file:line`, que se borra sin reemplazo directo: su propio
docblock confiesa que comprueba que la línea **exista**, no que **diga** lo que se afirma.

---

## Cinco de los 23 programas de conformidad NO SE EJECUTAN (medido el 16)

Los artefactos de `programs.rs` se escribieron el 15 y **cinco salieron con `REFUSED` en su sección
`── run ──`**. Se bendijeron y hubo que retirarlos: un golden que registra una negativa es
exactamente lo que el paso 8 existe para impedir —el ejemplo rancio con la luz verde—, y bendecirlo
convierte «no lo sabíamos» en «es lo esperado», que es peor que cualquiera de los dos. Sus
`compiled.txt` están borrados, así que el test se queda rojo por artefacto ausente hasta que alguien
decida. **Nada de esto era visible antes, porque los artefactos no existían.**

Tres causas, y la primera es una **discrepancia entre la superficie y el backend**:

| programa | qué lo tumba |
|---|---|
| `self-join`, `shop` | `join_key` (`fossil-df/src/plan.rs:234`) exige **el mismo nombre de columna en los dos lados**. `on = Node.parent == Other.id` y `on = ... user_id == ... id` son legales en la superficie que la documentación publica |
| `compound-key` | ni llega a esa comprobación: el `on` es `... and ...` y la puerta sólo acepta un `Eq` desnudo |
| `projection` | `Cast error: Cannot cast string '' to value of Date32` |
| `sightings` | `Json error: Not valid JSON: EOF while parsing a list` |

**La causa raíz de las tres primeras está aguas arriba y ya tiene nombre:** `lower.rs:1004` vacía
`ColRef.source`, así que `join_key` no puede saber qué lado es cuál y exigir nombres iguales es lo
único que *puede* hacer. Arreglar `join_key` sin devolverle el `source` no es posible.

Y ningún test de Rust ejecuta esos programas, así que **ninguno estaba rojo en ninguna parte**.

---

## Trampas medidas, que cuestan caro si se descubren tarde

- **`name = User.name` ya parsea hoy y se tira en silencio.** `parse_iri_expr` tolera un `IDENT`
  desnudo y `lower_property` acaba en `return None`, que `body.rs` se salta sin una palabra.
  Reescribir un fixture antes de tiempo no rompe: **pierde las propiedades sin decirlo**. Lo mismo
  con una cabecera sin CURIE — el mapeo desaparece del HIR sin diagnóstico.
- **`ExprId` está desalineado y este paso lo dispara.** `body.rs` sólo incrementa el contador con
  las propiedades que bajaron bien; `fossil-ide/src/hover.rs` calcula el `ExprId` por posición entre
  los hijos `PROPERTY` del CST. Una propiedad caída desplaza todo lo que va detrás.
- **`fossil-mir` se queda sin el IRI.** Hoy lo desnuda de la clave para escribir `rdf_uri`. Con
  clave desnuda tiene que bajar desde la forma. El nombre corto de columna ya lo calcula solo.
- **Seis implementaciones de «último segmento del IRI»**, y dos discrepan (unas parten por `#/`,
  otras también por `:`). La canónica ya vive en `fossil-graph-schema`.
- **`value_ty: None` no significa «cualquier valor».** `check.rs` hace
  `unwrap_or_else(|| Ty::new(db, TyKind::Iri))` — la expectativa más estricta que existe — mientras
  el doc-comment dice lo contrario. Una restricción que el descriptor no supo estrechar **rechaza
  un String**.
- **`resolve_target_shape` se traga los fallos con `.ok()?`**: documento ilegible, malformado, o
  forma no declarada son los tres `None`, indistinguible de «el programa no nombra documento». El
  caso de cabecera de ADR-0057 —`ex:Persn` mal escrito— es silencio.
- **El corpus de diagnósticos rodea la ruta de producción** (`# mode: helper-proven`). Desde que el
  checker lee la forma se puede pasar a la real.

---

## Ya muerto, y el árbol no se ha enterado — **la sección se ha enterado ella la última**

**Comprobada entera el 19, y tres de sus cinco afirmaciones eran ya falsas.** Es su propio título
vuelto contra ella, y queda escrito porque el modo de fallo importa más que la lista.

- ~~`Optional`, `Fn`, `Checker::check`, el atajo de clausura y `synthesize_closure`.~~ **Ya borrados**,
  en `3bda536` («una grafía por idea»), antepasado de HEAD. `TyKind` tiene siete variantes y ninguna
  es `Optional` ni `Fn`; lo que queda son lápidas (`check.rs:418,448,557,748,1536`, `ty.rs:89`). Los
  cinco tests se fueron con ellos.
- ~~`DefMap::lookup_prefix` sólo la llama su propio test.~~ **Borrada, en el mismo commit.** Las tres
  referencias que quedan son lápidas — y una de ellas es la línea 112 de este documento, que ya decía
  «no existe: borrada» mientras ésta seguía pidiendo borrarla.
- ~~S-OptCov la afirma un comentario y no está implementada.~~ **Refutada.** No son 19 menciones sino
  cuatro, y ninguna es lo que la afirmación describe: dos son lápidas correctas, la tercera es esta
  frase, y la cuarta es `book/typing.mdx:220`, donde la regla **está definida de verdad**, en un
  sistema coherente con S-Refl, S-Opt, S-IntFlt y S-SeqCov, en una página cuya cabecera dice que una
  regla escrita ahí y ausente de `crates/` es *trabajo pendiente, no un error de la página*. Borrar
  el nombre sería romper la referencia para que encaje con la implementación, que es al revés.
- `ShapeBinding.closed` — **ya borrado también**, y su lápida en `fossil-graph-schema/src/shapes.rs`
  explica por qué cablearlo no habría cambiado nada: una clave es un nombre desnudo resuelto contra
  esa tabla, así que un predicado que no declara ya es un error, incondicionalmente. *«Un flag que
  sólo puede elegir el comportamiento que ya ocurre no es un flag.»*
- **`TargetShapeError`: cuatro de cinco borradas el 19**, probadas inalcanzables con catorce
  programas reales por `resolve_target_shape` — ni un `Err` en los trece que fallan. Queda
  `NoDocument`, igual de muerta y **bloqueada por propiedad, no por duda**: colapsar
  `Result<Option<_>, _>` a `Option<_>` toca tres líneas de `fossil-ide`.

**Lo que sí está vivo y no lo había listado nadie**, encontrado por la misma sonda:
`DefMap::shape_binding_for` busca por **IRI de forma**, así que dos bindings `type` que resuelvan al
mismo IRI devuelven en silencio el documento del primero. Probado: dos documentos declarando ambos
`http://example.org/Person`, mapeo sobre el segundo, y vuelven las restricciones del primero. Es
exactamente el caso que la ligadura posicional presume de distinguir — «dos documentos que declaran
`Person` se distinguen por sus etiquetas locales» — y el arreglo necesita el nombre local, que se ha
perdido cuando `resolve_target_shape` corre.

Y estos, con llamantes sólo en sus propios tests, sin borrar todavía: `DefMap::lookup_source`,
`DefMap::lookup_source_schema`, `provenance::mapping_at` (re-exportada en `lib.rs:90`),
`ProvenanceKind::SynthesizedClosureRendering`, e `infer::resolve_source_row` — ésta última con cinco
comentarios que la nombran como parte de la ruta de typecheck, lo cual ya es falso.

---

## La documentación, en paralelo

Dos productos, dos sitios:

- **`apps/docs`** — el lenguaje. `/design` (los núcleos de tipos, providers, inferencia, álgebra,
  identidad, tooling) y `/book` (la anatomía de un programa, forma Gluon).
- **`apps/corpus`** — el grafo. El formato y sus **convenciones con guardias ejecutables** (decisión
  5), los seis verbos, streaming, larger-than-RAM.

Antes del borrado de `decisions/` hay que **cosechar** lo que no está en ningún otro sitio: las
mediciones, el arte previo consultado, el campo «qué revertiría esto», y la capa de auditoría no
versionada de `.planning/` — el libro de bajas de keasy, las evaluaciones de dependencias, el
registro de ejecución con SHAs. `.planning/` está en `.gitignore` y tres ficheros versionados
apuntan dentro.

---

## Deuda anotada, no pagada

- ~~**`UNARY_EXPR` sigue siendo un agujero.**~~ **Caducado, comprobado el 14.** La variante existe
  (`HirExpr::UnaryOp { op, operand }`, `lower.rs:369`) y `UNARY_EXPR` desciende (`lower.rs:1663`).
  Se cerró con el trabajo de superficie y este párrafo no se enteró.
- **Los verbos como catálogo tocan el checker**, y eso no está dimensionado.
- **`select` cuando la relación viene de un join** no tiene sitio (ADR-0059 lo deja abierto).
- **`io` sólo nombra la entrada.** El destino no tiene sintaxis ninguna.
- **El constructor de aristas con dos huecos** no está especificado: ni el orden de ligadura, ni si
  el argumento es la entrada del hueco o su valor terminado.
- **Las destructuraciones cortas** (nombrar menos miembros de los que el documento declara) no están
  especificadas.
- **`crates/fossil-mcp` está mal colocado y mal llamado**: es la cara IA del lado grafo. **Ya tiene
  dueño**: F8, donde el corte en dos árboles le da su sitio en vez de sólo moverlo.

---

## Estado al cierre de la sesión del 2026-08-13

### Lo que se descubrió y hay que arrastrar

**Seis tests de `fossil-cli` ejecutan un binario obsoleto, y uno es el invariante duro.**
`walking_skeleton.rs:52`, `run_rdf.rs:44`, `run_source_pipeline.rs:39`, `run_w0b.rs:42`,
`refs.rs:35` y `check_diagnostics.rs:48` clavan la ruta `<repo>/target/debug/fossil`. Con
`CARGO_TARGET_DIR` puesto —que es lo que exigen las instrucciones del repo— la compilación va a otro
sitio y esa ruta guarda lo que quedara allí. Medido: **29 horas de diferencia**, anterior a la
reescritura del parser, a `clean`→`str`, al registro de proveedores, a `@rename` y al constructor de
aristas. **Todo verde que hayan dado esos seis es sobre un compilador que nadie había editado.** El
arreglo es `env!("CARGO_BIN_EXE_fossil")`, que ya se aplicó a los tres de `fossil-lsp`.

**La puerta de CI de 400 ms medía el camino de error.** `didchange_budget` construía su db sobre
`NativeSystem`, cuya tabla de proveedores **no lee tipos**: 0 predicados resueltos y **32
diagnósticos empujados por el acumulador en cada pulsación**. Medir el camino real **no es más lento,
es ~8 % más rápido** (0,169 → 0,155 ms), porque la decodificación está memoizada por el `SourceFile`
del documento y editar el programa la reejecuta cero veces, mientras el camino que fallaba pagaba 32
empujes. **La forma del arreglo es lo que hay que copiar**: la puerta asserta `RESOLVED_PREDICATES ==
60` **antes de arrancar el reloj**, para que un cambio que deje de resolver el contrato falle a gritos
en vez de salir halagadoramente más rápido.

**Y el fixture de `MAX_REEXECUTIONS = 18` tiene el mismo agujero y le vale el mismo arreglo**: no
nombra documento de forma, así que sale por `NoDocument` sin tocar `shape_document`. 18 es verde y no
dice nada sobre la ruta nueva.

**Tres tests de `fossil-lsp` estaban verdes sobre un binario de dos días** por la misma causa, y al
arreglarlo apareció `lsp_hover_smoke` genuinamente roto. `typecheck_mapping` devuelve `Err` cuando el
contrato no resuelve, y eso vacía la tabla que lee el hover.

### Lo que quedaba en vuelo

Diez frentes, todos con su encargo escrito y reanudables: el binding derivado (nueve programas, una
causa), la aritmética y el unario, la identidad por tipo, la poda y las tres nociones de `ExprId`, el
registro de proveedores y el borrado de CSVW, la regla única de rutas, `@rename` y la arista, el
corpus de conformidad, las citas por nombre de producción, y en kanzo-ui la capa de color.

### El árbol, al cierre

`cargo check --workspace` rojo en dos sitios con dueño conocido: `check.rs`/`provenance.rs` por
variantes de enum nuevas sin brazo (`CmpOp::{Add,Sub,Mul,Div,Rem}`, `HirExpr::{FloatLit,BoolLit,
UnaryOp}`, `synth_unary`), y `check_tests.rs` por el campo `source_scope`. Las dos son mitades de un
cambio en curso, no regresiones.
