# ADR 0058: Una identidad por tipo, y el programa la declara en un slot del lenguaje

**Date:** 2026-08-11
**Status:** proposed
**Decider:** Ángel Iglesias (Kanzo)
**Cite:** ADR-0057 (séptima enmienda §4, la frontera atributo/sintaxis; novena, la referencia cualificada); ADR-0055 (el checker lee el documento de forma); ADR-0042 (la cámara se direcciona); JLS §9.7.1; Jakarta Persistence 3.2 §12.2.3.8 y `EntityBinder.java:1154-1163`; JSON-LD 1.1 §keywords; PLDI 2016 «Types from data» §6.3 y §6.5; Apollo Federation v1 `KEY_NOT_SPECIFIED` y su tabla de códigos retirados en v2; W3C R2RML §9.1.1 y Direct Mapping; `ObjectStringTemplateFunctionSymbolImpl.java:198` (Ontop); Xiao et al., ESWC 2018.

## Context

La identidad del sujeto es lo único del lenguaje que no cabe en ningún sitio limpio, y
lleva tres formas distintas desde que se planteó.

El documento de forma **no la declara**. Un `.shex` describe los predicados de un nodo, y en RDF
el sujeto **es** el nodo, no una propiedad suya — así que no hay slot al que asignar. LinkML
(`identifier: true`), Prisma (`@id`) y GraphQL (`@key`) sí lo declaran; ShEx no puede. Esa
asimetría no es un defecto de ShEx: es que en un grafo de propiedades la identidad **se señala** y
en RDF **se construye**, porque un IRI no está en el CSV y hay que fabricarlo.

Meterla en un atributo —`@subject(iri = "…/{u.id}")`— es lo que hay hoy, y no se sostiene. La
séptima enmienda §4 ya tuvo que declararlo *«sintaxis con sigilo, no un atributo»* para salvar su
propia ley de «sólo constantes», y una grafía cuya naturaleza hay que explicar cada vez es lo que
la regla 2 de la casa prohíbe. La revisión de siete lenguajes lo confirma sin una sola excepción:
**ningún lenguaje admite una expresión por instancia en un argumento de atributo.** En Java es un
truco sintáctico deliberado (JLS §9.7.1: *«a syntactic trick to prevent assignment expressions as
element values»*); `@Id` de JPA está literalmente vacío (`public @interface Id {}`) y expresa la
identidad **por dónde está puesto**; Prisma no tiene lenguaje de expresiones, sino nueve nombres de
función cerrados en el motor. Y el experimento de control es Ecto: Elixir **sí** evalúa expresiones
en atributos de módulo, así que `default: DateTime.utc_now` compila — y está mal, produce un único
valor congelado para todas las filas, y Ecto gasta un párrafo de documentación avisando de un error
que en Java ni se puede escribir. Dado poder total, acabó enrutando la generación por fila a través
de una indirección de datos inertes.

El arte previo prescribe la salida cuando la expresión por fila es genuinamente necesaria: **su
propia construcción con nombre**, no ensanchar la ranura del atributo — `GENERATED ALWAYS AS` de
SQL, `GeneratedField` de Django. Y el modo de fallo a evitar tiene nombre: el `@PreAuthorize("…")`
de Spring, donde la expresión vuelve como cadena sin comprobar y se pierden las garantías.

Queda entonces **dónde** vive y **si dos mapeos del mismo tipo pueden discrepar**. Lo segundo no
tiene precedente en el mundo de los mapeos: ni R2RML, ni RML, ni sus mapeadores, ni la literatura
de calidad lo comprueban, y la comunidad cerró la petición como fuera de alcance. Ontop **tiene el
detector** —plantillas incompatibles ⇒ rama podada (`ObjectStringTemplateFunctionSymbolImpl.java:198`)—
y lo cablea sólo al optimizador; Xiao et al. (ESWC 2018) *asumen* la propiedad: *«it is desired that
each IRI can be constructed by at most one IRI template, and we make such assumption here»*.

Y hay un contraejemplo que hay que responder, no ignorar: **Apollo Federation llevaba esta
comprobación y la retiró.** `KEY_NOT_SPECIFIED` existía en v1; v2 la eliminó publicando el motivo —
*«Each subgraph can declare a key independently of any other subgraph»*. Verificado en vivo: dos
subgrafos con claves disjuntas componen con «COMPOSED OK — no errors, no hints», mientras que un
comentario de documentación discrepante sí dispara un aviso.

## Decision

**La identidad es un slot que posee el lenguaje, se escribe como una asignación, y es única por
tipo.**

1. **`@subject = <expr>`**, primera línea del cuerpo del mapeo. No es un atributo: es una
   asignación, y su parte derecha usa la gramática de expresiones ordinaria. El `@` marca el
   espacio de nombres del lenguaje frente a los nombres desnudos que declara la forma — el criterio
   de Django («*model metadata is anything that's not a field*») con la solución de JSON-LD, que
   reserva `@`+ALPHA por la misma razón: las claves del usuario son arbitrarias y no puedes
   apropiarte de `id`. Mueren `@subject(iri = …)`, la palabra reservada `iri` y `PropertyKey::Iri`.

2. **Dos obligaciones que las demás propiedades no tienen**, y son consecuencia de que un IRI
   denota inequívocamente:
   - **Determinista y acotada a la fila.** Una identidad que dependa de algo que no es el dato no
     es reproducible. Esto es una comprobación del checker, no una regla de estilo.
   - **Única por tipo.** Todos los mapeos que producen `T` declaran el mismo `@subject`; si no,
     **error**, con el diagnóstico nombrando los dos mapeos y las dos plantillas.

3. **El error es error, no aviso.** Un aviso sobre identidad se ignora y el resultado son dos
   entidades donde había una.

**Por qué el caso de Federation no aplica.** Sus dos `@key` sobre un tipo son dos maneras de
*localizar* la misma entidad —claves alternativas, como dos `UNIQUE`—; la identidad sigue siendo
una. Aquí se comprobaría que dos mapeos no **acuñen identidades distintas**, que no es una clave
alternativa sino una contradicción. Y sobre todo: Apollo **no puede** comprobarlo aunque quiera,
porque sus subgrafos se despliegan por separado y nadie tiene el conjunto delante. fossil es
write-once/read-many y compila un programa entero de una vez, así que la comprobación global es
viable precisamente porque la topología es la contraria. El W3C da el contraste dentro de su propia
casa: Direct Mapping deriva la identidad de la tabla y su clave, y la coincidencia se cumple **por
construcción**; R2RML abrió la plantilla libre por mapeo y ahí nace el problema que nadie comprueba.

## Consequences

**Desaparece una excepción del lenguaje.** No hay «sintaxis con sigilo» que explicar: hay slots del
lenguaje, marcados con `@`, a los que se asigna. Y el mecanismo generaliza — `@graph` para los
quads, que `architecture.md` reclama como ciudadanos de primera clase y hoy no están en el esquema
del corpus por ninguna parte, entra por la misma puerta sin abrir una segunda.

**El `.shex` no tiene que declarar nada nuevo**, que era la objeción que tumbó la alternativa de
tratar la identidad como una propiedad más.

**Adivinar pasa a comprobar.** Hoy una arista se deduce comparando *esqueletos* de plantilla de IRI
entre mapeos (`fossil-mir/src/skeleton.rs`), un marcador `\u{1}` de por medio, a ver si casan. Con
una identidad por tipo, esa comparación se convierte en búsqueda, y `subject_skeletons` — que existe
sólo porque la regla está repetida por mapeo — pierde su razón de ser.

**El checker gana dos obligaciones nuevas** y una de ellas es de pureza, que hoy no tiene ninguna
otra expresión. Eso es código nuevo en el sitio correcto, no una carga escondida.

**Se paga rigidez.** Dos fuentes que describan poblaciones disjuntas del mismo tipo con esquemas de
identidad distintos dejan de poder expresarse. No hay ningún caso así en el corpus hoy.

**La guía de Linked Data juega en contra y hay que decirlo**: *«Agents making use of URIs SHOULD NOT
attempt to infer properties of the referenced resource»*, y la unidad de política de acuñación en
esa literatura es el publicador, no el tipo. Aquí la elegimos por tipo porque el compilador es la
autoridad única del programa.

**Lo que revertiría esta decisión**, en orden:

1. **Un caso legítimo de discrepancia**: dos mapeos del mismo tipo que **deban** acuñar distinto —
   poblaciones disjuntas sin esquema de identidad común. Uno real baja la comprobación de error a
   aviso.
2. **Que la distinción con Federation no se sostenga** — es decir, que aparezca una topología donde
   fossil componga mapeos de autoridades independientes. Ahí su argumento pasa a ser el nuestro.
3. **Invertibilidad** (`rr:inverseExpression`): si algún día se quiere acceso virtual/OBDA, una
   plantilla por tipo no basta y hace falta la expresión inversa.

**Lo que NO se verificó:** dos taxonomías de calidad de mapeos R2RML tras muro de pago (Randles &
O'Sullivan, iiWAS 2020; Crotti Junior et al., i-Semantics 2019), que son el sitio más probable donde
exista ya una categoría nombrada de «plantilla de IRI inconsistente». Y no hay ningún estudio
publicado que mida con qué frecuencia un conjunto real de mapeos acuña dos IRIs para una entidad —
el daño está documentado en material docente de Ontop, no en un informe de fallo.
