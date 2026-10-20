"""The cut of GTFS-Madrid-Bench's upstream CSV mapping that fossil's program defines.

    python fragment.py upstream.rml.ttl > gtfs.rml.ttl

`upstream.rml.ttl` is `mappings/gtfs-csv.rml.ttl` from oeg-upm/gtfs-bench, unchanged. This
removes the predicate-object maps of the five predicates `gtfs.shex` leaves out, and points
each source at `data/` beside the mapping instead of `/data/`. Nothing else changes: every
join the upstream mapping makes, it still makes. Needs `rdflib` (Morph-KGC depends on it).
"""

import sys

import rdflib

RR = rdflib.Namespace("http://www.w3.org/ns/r2rml#")
RML = rdflib.Namespace("http://semweb.mmlab.be/ns/rml#")
GTFS = rdflib.Namespace("http://vocab.gtfs.org/terms#")
LEFT_OUT = {GTFS.routeType, GTFS.locationType, GTFS.wheelchairAccessible, GTFS.pickupType,
            GTFS.dropOffType}


def predicate(g, pom):
    p = g.value(pom, RR.predicate)
    return p if p is not None else g.value(g.value(pom, RR.predicateMap), RR.constant)


def drop(g, node):
    """`node` and every blank node only it reaches."""
    for p, o in list(g.predicate_objects(node)):
        g.remove((node, p, o))
        if isinstance(o, rdflib.BNode) and not any(g.subjects(None, o)):
            drop(g, o)


def main():
    g = rdflib.Graph().parse(sys.argv[1], format="turtle")
    removed = 0
    for tm, pom in list(g.subject_objects(RR.predicateObjectMap)):
        if predicate(g, pom) in LEFT_OUT:
            g.remove((tm, RR.predicateObjectMap, pom))
            drop(g, pom)
            removed += 1
    assert removed == 6, f"expected the six maps of five predicates, removed {removed}"
    for ls, src in list(g.subject_objects(RML.source)):
        g.set((ls, RML.source, rdflib.Literal(str(src).lstrip("/"))))
    sys.stdout.write(g.serialize(format="turtle"))


if __name__ == "__main__":
    main()
