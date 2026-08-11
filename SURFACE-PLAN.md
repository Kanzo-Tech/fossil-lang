# La superficie: qué falta y en qué orden

**Escrito 2026-08-11.** Las decisiones están en `decisions/0058-…` (la identidad) y
`decisions/0059-…` (todo lo demás, con el programa objetivo). **Esto es sólo el orden de
construirlo** — si algo aquí contradice a un ADR, gana el ADR.

Cada paso dice **qué lo desbloquea** y **qué prueba que está hecho**, porque este repo ya tiene
el precedente de dar algo por terminado porque estaba documentado.

---

## Hecho (2026-08-11)

| | commit |
|---|---|
| Referencia cualificada `users.name` | `f9ee991` |
| Interpolación `"{expr}"`, y el parser de plantillas sale de MIR | `3de025f` |
| `@subject` parsea — primera producción que acepta `@` | `a0d9bfa` |
| Los cuatro paquetes de UI fuera (7.865 borrados) | `873cbc0` |
| Poda 1: `$VAR`, `<<s p o>>` | `ee6fdef` |
| Los 6 verbos RDF de la stdlib y 2 tipos del retículo | `4213e74` |
| ADR-0058 — la identidad | `8133424` |
| Cita ADR-0050 → 0051 (y en `Cargo.toml`) | `1a4af20` |
| ADR-0059 — la superficie entera | `62fe56c` |
| `timeout_ms`, muerto, y su comentario que mentía | `329d7ef` |
| **`fossil-mir` deja de conocer ShEx** | `0e6898d` |
| Poda 2: `in`, `&`, anotaciones, registros, `use`, `T_COLON` + `PAREN_EXPR` arreglado | `62ed84f` |
| `temp_dir` fijo del CLI — dos suites a la vez ya no se pisan | `6b597ea` |

---

## El árbol

```
┌─ CORTE DEL DESCRIPTOR DE SALIDA ──────────────────── la raíz
│  fossil-hir deja de enlazar shex_ast y rudof_iri.
│  Accesor ambiente en `System` — NUNCA en la clave de una consulta.
│  (fossil-mir ya está cortado: 0e6898d)
│
├──▶ CLAVES DESNUDAS   name = …   en vez de   ex:name = …
│    │  necesita el corte: sin él, «la forma conoce sus predicados»
│    │  significa «el documento ShEx los conoce», y cambias un
│    │  compromiso con RDF por otro escrito distinto
│    └──▶ ─┐
│          ├──▶ FUERA `prefix`
├──▶ CABECERA   Users : Person from Adults      ─┘   (sólo cuando las dos
│    el tipo ligado por posición, sin IRI              lo dejan sin trabajo)
│    │
│    └──▶ CABECERA CON TUBERÍA + PASO 4
│         la comprobación de fila deja de ser igualdad
│         y pasa a ser búsqueda en ámbito (el join deja DOS filas)
│
├──▶ IDENTIDAD  @subject = expr        ← ADR-0058, independiente del corte
│    única por tipo + determinista y acotada a la fila
│    └─ mata subject_skeletons, PropertyKey::Iri, KW_IRI
│
├──▶ TIPOS DE ENTRADA CON NOMBRE   User := io.csv(…)
│    liga tipo Y relación con un nombre
│    └──▶ REFERENCIA POR TIPO   User.age
│         └─ mata `as` como forma normal (queda para el self-join),
│            la fila anónima, y con ella la síntesis implícita de
│            clausuras: expr_contains_free_field_refs,
│            rewrite_field_refs_to_row_dot y synthesize_closure
│            existen SÓLO porque el predicado no nombraba su fila
│
├──▶ VERBOS COMO ENTRADAS DE CATÁLOGO
│    where/select/join dejan de tener producciones;
│    RegistryEntry gana tipo de receptor
│    └─ NO DIMENSIONADO: toca el checker, no sólo el catálogo
│
├──▶ `:=` también para `type`     (trivial, suelto)
└──▶ ARISTA CON CONSTRUCTOR   buyer = Person(User.email)   (suelto)

                    ▼  todo lo anterior
        D · los 68 fixtures, UNA vez, a la sintaxis final
                    ▼
        E · borrar las grafías viejas
            FieldRef, TEMPLATE, ${prefix:}, iri, CURIEs
```

**La ruta crítica:** corte → claves desnudas → cabecera → `prefix` → D → E.
Todo lo demás cuelga a los lados y puede ir en paralelo.

---

## Tres cosas que se pierden fácil y cuestan caro

**D son 68 ficheros `.fossil` versionados, no 35.** La novena enmienda de ADR-0057 contó 35 de 46
sobre un árbol que ya no es éste; hoy son 83 sin contar `packages/examples/dist`, que está en
`.gitignore`. Reparto: 36 en `examples/src`, 14 en tests de `fossil-syntax`, 14 en `fossil-hir`,
4 sueltos. **Corregir esa cifra en el ADR cuando D aterrice.**

**D va al FINAL, no en el paso 4.** La escalera A→E original asumía que el destino era sólo
referencia cualificada + interpolación + `@subject`. El destino creció: reescribir antes de que
estén las claves desnudas y la cabecera significa reescribir dos veces, y «los fixtures una sola
vez» era el motivo entero de la escalera.

**`prefix` va DESPUÉS del corte del descriptor.** Antes de eso no es una decisión, es un cambio de
ropa.

---

## Deuda anotada, no pagada

- **`UNARY_EXPR` sigue siendo un agujero.** `not …` y el menos unario parsean y caen en el
  diagnóstico de respaldo. Necesita una variante de `HirExpr` que no existe: es decisión, no brazo.
  (Su gemelo `PAREN_EXPR` ya está arreglado, `62ed84f`.)
- **Los verbos como catálogo tocan el checker** y eso no está dimensionado.
- **`select` cuando la relación viene de un join** no tiene sitio. Anotado como pregunta abierta en
  ADR-0059.
- **`io` sólo nombra la entrada.** El destino no tiene sintaxis ninguna.
- **El corpus de diagnósticos sigue en `# mode: helper-proven`** — rodea la ruta de producción, y
  desde que el checker lee la forma se puede pasar a la real.
- **Las citas `grammar.bnf:NNN` de todo el árbol están podridas** —ADR-0057, `rdf12.mdx`, los
  doc-comments del parser— porque nada las comprueba. `apps/docs/content.test.ts` ya verifica que
  las RUTAS que cita una página existan; extenderlo a números de línea es barato.
- **`fossil-mir` enhebra la tabla de prefijos por 6 funciones y no la lee jamás** — cero llamadas a
  `lookup_prefix` en ese crate. ~47 líneas que desaparecen solas cuando caiga `prefix`.

---

## Fuera de la superficie, decidido y sin construir

**ADR-0040 sigue en `proposed`** y tiene **dos hechos falsos dentro**: dice que `@fossil-lang/viewer`
ya declara `@kanzo-tech/ui` (declara `@fossil-lang/ui`; no existe ni una dependencia `@kanzo-tech/*`
en este repo), y cuenta `playground.kanzo.dev` como consumidor cuando `e62299c` borró
`packages/playground` **y** `apps/landing` enteros. Lo segundo REFUERZA la decisión. Falta también
su fila en `decisions/README.md`.

**El lado Rust del borrado de UI:** `crates/fossil-wasm/src/tokenize.rs` y
`packages/types/src/token.ts` siguen llevando el pin entre los discriminantes del lexer y
`tags.ts` — un invariante que **ningún test comprueba** y que un reordenamiento rompería repintando
el editor en silencio. Muere, no se muda: `semanticTokens/full` ya está servido en los dos
servidores y no lo consume nadie. Antes hay que cerrar tres huecos en `fossil-ide/src/semantic.rs`
(uno ya cerrado en `ee6fdef`: las cadenas interpoladas; quedan la puntuación y una ranura `ty::TYPE`
declarada que nadie emite).

**CSVW se borra entero.** El árbol ya lo empezó: `resolve_binding_row` prueba primero el
`InferredDescriptor` del host y emite `D-CSVW-DEPRECATED` si además hay un `schema =` explícito.
Cuidado con el límite: `schema =` **no** muere entero — lo comparten `*.csvw.json` (se va) y
`*.shex` de las fuentes `io.rdf(...)` (se queda).

**`KEASY-MIGRATION-MAP.md`** (raíz de este repo) tiene los 11 ficheros de keasy y el trabajo real:
el adaptador filas→buffers densos, que es problema del host y no del paquete.
