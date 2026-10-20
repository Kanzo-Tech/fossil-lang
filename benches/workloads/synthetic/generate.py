"""Synthetic inputs for the benchmark, deterministic in (workload, rows, seed).

Each workload's files have exactly the names, delimiters and headers its
program in `docs/programs/<workload>` reads, so the program runs unchanged; the
values are only scaled. `rows` is the size of the workload's main source, and
the others are derived from it as the comment on each generator says.

Every value is chosen so that the three engines are asked the same question:
no key repeats (a graph holds a set of triples, and a duplicate would test
deduplication instead of mapping), every foreign key resolves, and `shop`'s
users are all adults, so its `where` keeps every row and a mapping language
with no filter — RML-Core has none — defines the same graph.
"""

import argparse
import random
from pathlib import Path

FIRST = ["Ada", "Linus", "Grace", "Alan", "Barbara", "Edsger", "Frances", "Tony",
         "Margaret", "Dennis", "Radia", "Ken", "Hedy", "Niklaus", "Leslie", "Donald"]
LAST = ["Lovelace", "Torvalds", "Hopper", "Turing", "Liskov", "Dijkstra", "Allen",
        "Hoare", "Hamilton", "Ritchie", "Perlman", "Thompson", "Lamarr", "Wirth"]
WORDS = ["arrived", "warm", "quick", "late", "broken", "perfect", "as", "described",
         "would", "buy", "again", "box", "was", "damaged", "great", "value"]


def name(rng):
    return f"{rng.choice(FIRST)} {rng.choice(LAST)}"


def write(path, header, rows, sep=","):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as f:
        f.write(sep.join(header) + "\n")
        for row in rows:
            f.write(sep.join(str(v) for v in row) + "\n")


def hello(out, n, rng):
    """`people.csv`: n people."""
    write(out / "data/people.csv", ["id", "name"],
          ((i, name(rng)) for i in range(1, n + 1)))


def reviews(out, n, rng):
    """`users.csv`: n/2 users; `reviews.csv`: n reviews, each by a random user."""
    users = max(1, n // 2)
    write(out / "data/users.csv", ["id", "email", "name"],
          ((i, f"user{i}@shop.example", name(rng)) for i in range(1, users + 1)))
    write(out / "data/reviews.csv", ["id", "email", "body"],
          ((f"r{i}", f"user{rng.randint(1, users)}@shop.example",
            " ".join(rng.choice(WORDS) for _ in range(6)))
           for i in range(1, n + 1)))


def shop(out, n, rng):
    """`users.csv`: n/2 users, all adults; `orders.csv`: n orders, each by a random user."""
    users = max(1, n // 2)
    write(out / "data/users.csv", ["id", "email", "name", "age"],
          ((i, f"user{i}@shop.example", name(rng), rng.randint(18, 90))
           for i in range(1, users + 1)))
    write(out / "data/orders.csv", ["id", "user_id", "amount"],
          ((100 + i, rng.randint(1, users), f"{rng.randint(100, 99999) / 100:.2f}")
           for i in range(1, n + 1)))


def pairs(rng, count, left, right, distinct_ends=False):
    """`count` distinct pairs over 1..left × 1..right."""
    seen = set()
    count = min(count, left * right - (left if distinct_ends else 0))
    while len(seen) < count:
        a, b = rng.randint(1, left), rng.randint(1, right)
        if distinct_ends and a == b:
            continue
        seen.add((a, b))
    return sorted(seen)


def relationship(out, n, rng):
    """LDBC-SNB's layout: n persons, n/100 places, n/10 tags (at least 10 each),
    2n `knows` pairs and 2n `hasInterest` pairs, `|`-delimited."""
    places, tags = max(10, n // 100), max(10, n // 10)
    write(out / "data/place.csv", ["id", "name"],
          ((i, f"Place {i}") for i in range(1, places + 1)), "|")
    write(out / "data/tag.csv", ["id", "name"],
          ((i, f"Tag {i}") for i in range(1, tags + 1)), "|")
    write(out / "data/person.csv", ["id", "firstName", "place"],
          ((i, rng.choice(FIRST), rng.randint(1, places)) for i in range(1, n + 1)), "|")
    write(out / "data/person_knows_person.csv", ["Person.id", "Person.id_1", "creationDate"],
          ((a, b, f"20{rng.randint(10, 25)}-{rng.randint(1, 12):02}-{rng.randint(1, 28):02}")
           for a, b in pairs(rng, 2 * n, n, n, distinct_ends=True)), "|")
    write(out / "data/person_hasInterest_tag.csv", ["Person.id", "Tag.id"],
          pairs(rng, 2 * n, n, tags), "|")


WORKLOADS = {"hello": hello, "reviews": reviews, "shop": shop, "relationship": relationship}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("workload", choices=WORKLOADS)
    ap.add_argument("rows", type=int)
    ap.add_argument("out", type=Path)
    ap.add_argument("--seed", type=int, default=20261019)
    a = ap.parse_args()
    WORKLOADS[a.workload](a.out, a.rows, random.Random(f"{a.seed}/{a.workload}/{a.rows}"))


if __name__ == "__main__":
    main()
