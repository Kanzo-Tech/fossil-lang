"""GTFS-Madrid-Bench's upstream CSV mapping, reading its data beside the mapping.

    python fragment.py upstream.rml.ttl > gtfs.rml.ttl

`upstream.rml.ttl` is `mappings/gtfs-csv.rml.ttl` from oeg-upm/gtfs-bench, unchanged. This
points each source at `data/` beside the mapping instead of `/data/`, and nothing else
changes: every predicate and every join the upstream mapping makes, it still makes. Needs
`rdflib` (Morph-KGC depends on it).
"""

import sys

import rdflib

RML = rdflib.Namespace("http://semweb.mmlab.be/ns/rml#")


def main():
    g = rdflib.Graph().parse(sys.argv[1], format="turtle")
    for ls, src in list(g.subject_objects(RML.source)):
        g.set((ls, RML.source, rdflib.Literal(str(src).lstrip("/"))))
    sys.stdout.write(g.serialize(format="turtle"))


if __name__ == "__main__":
    main()
