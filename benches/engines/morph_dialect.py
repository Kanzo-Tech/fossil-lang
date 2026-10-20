"""Morph-KGC's spelling of a logical source.

The mappings in `rml/` describe each file as RML-IO does — a `rml:RelativePathSource`,
or a `csvw:Table` where the dialect is not a comma — and Morph-KGC 2.10 reads
neither: `rml:source` is a path literal to it, read as comma-separated, and its
fallback for another delimiter (pandas' sniffer) takes the `.` of LDBC's
`Person.id` header for the delimiter. So a comma file becomes the path, and a
`csvw:Table` becomes Morph-KGC's own "tabular view", an `rml:query` it runs on
DuckDB's CSV reader with the dialect spelled out. Nothing else is touched, so
both processors run the same triples maps.
"""

import re
import sys

PATTERNS = [
    (r'rml:source \[ a rml:RelativePathSource ; rml:root rml:MappingDirectory ; rml:path ("[^"]+") \]',
     r"rml:source \1"),
    (r'rml:source \[ a csvw:Table ; csvw:url "([^"]+)" ;\s*'
     r'csvw:dialect \[ a csvw:Dialect ; csvw:delimiter "(.)" \] \]',
     "rml:query \"SELECT * FROM read_csv('\\1', delim = '\\2', header = true, all_varchar = true)\""),
]


def rewrite(text):
    for pattern, spelling in PATTERNS:
        text = re.sub(pattern, spelling, text)
    if "rml:source [" in text:
        raise SystemExit("a logical source this rewrite does not know")
    return text


if __name__ == "__main__":
    sys.stdout.write(rewrite(open(sys.argv[1], encoding="utf-8").read()))
