# El plan de construcción

**Reescrito 2026-08-11**, después de auditar el corpus entero. Las decisiones vivas son
`decisions/0058-…` (la identidad) y `decisions/0059-…` (la superficie, con el programa objetivo).
**Esto es el orden de construirlo** — si algo aquí contradice a un ADR alto, gana el ADR.

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
- **`resolve_target_shape` ya NO se traga los fallos** — cinco variantes nombradas de
  `TargetShapeError`, arreglo documentado en `shapes.rs:35-41`. Lo que queda es que
  `completion.rs:181` y `hover.rs:144` colapsan las cinco causas a «nada».
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
│     mató subject_skeletons, PropertyKey::Iri, KW_IRI
│
├──▶ 2 · CABECERA   Users : Person from Adults          ← AQUÍ
│    │  ShapeExpr := IDENT, resuelto contra los nombres
│    │  que introduce `type { … }`. Muere el CURIE en cabecera.
│    └──▶ 3 · FUERA `prefix`
│           el lexer pierde `prefix`, ABS_IRI, `${ex:}`;
│           el HIR pierde PrefixEntry y la expansión
│
├──▶ 5 · TIPOS DE ENTRADA CON NOMBRE   User := io.csv(…)
│    └──▶ REFERENCIA POR TIPO   User.age
│           muere FieldRef, y con él las tres piezas de
│           la clausura implícita
│
├──▶ 6 · VERBOS COMO CATÁLOGO   (+ muere `|>`)
│    └─ RegistryEntry gana tipo de receptor; el despacho
│       deja de ir por cadena. Toca el checker.
│
└──▶ 7 · `:=` para `type`; ARISTA CON CONSTRUCTOR; `@rename`
                    ▼
        8 · LOS PROGRAMAS DE LA DOCUMENTACIÓN SON EL CORPUS
            los 18 se compilan y se guarda su salida o su
            diagnóstico; reescritura única de lo que sobreviva
                    ▼
        9 · UNA SOLA REFERENCIA, Y ES LA BASELINE
```

**Ruta crítica:** 2 → 3 → 5 → **7** → 6 → 8 → 9.

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

## Ya muerto, y el árbol no se ha enterado

`Optional` y `Fn` no se construyen en ningún sitio de producción, así que `Checker::check`, el
atajo de clausura implícita y `synthesize_closure` son **inalcanzables desde `typecheck_mapping`** —
sólo los llaman cinco tests. ADR-0059 los programa matar; ya están muertos. S-OptCov la afirma un
comentario y no está implementada. `ShapeBinding.closed` se puebla y no lo lee nadie.
`DefMap::lookup_prefix` sólo la llama su propio test.

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

- **`UNARY_EXPR` sigue siendo un agujero.** Necesita una variante de `HirExpr` que no existe: es
  decisión, no brazo.
- **Los verbos como catálogo tocan el checker**, y eso no está dimensionado.
- **`select` cuando la relación viene de un join** no tiene sitio (ADR-0059 lo deja abierto).
- **`io` sólo nombra la entrada.** El destino no tiene sintaxis ninguna.
- **El constructor de aristas con dos huecos** no está especificado: ni el orden de ligadura, ni si
  el argumento es la entrada del hueco o su valor terminado.
- **Las destructuraciones cortas** (nombrar menos miembros de los que el documento declara) no están
  especificadas.
- **`crates/fossil-mcp` está mal colocado y mal llamado**: es la cara IA del lado grafo.

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
