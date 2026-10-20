"""fossil's graph as N-Triples: run the script `fossil-bench --triples-sql` wrote on
DuckDB and print the rows of its `triples` view, one triple a line.

    python readback.py <script.sql> <out.nt>

Needs the `duckdb` Python package (Morph-KGC depends on it), whose Parquet
reader is built in.
"""

import sys

import duckdb


def term(kind, value, datatype, lang):
    if kind == "I":
        return f"<{value}>"
    if kind == "B":
        return f"_:{value}"
    text = value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r")
    if lang:
        return f'"{text}"@{lang}'
    if datatype:
        return f'"{text}"^^<{datatype}>'
    return f'"{text}"'


def main():
    script, out = sys.argv[1:3]
    con = duckdb.connect()
    con.execute(open(script, encoding="utf-8").read())
    with open(out, "w", encoding="utf-8") as f:
        for sk, s, p, ok, o, dt, lang in con.execute("SELECT * FROM triples").fetchall():
            f.write(f"{term(sk, s, '', '')} <{p}> {term(ok, o, dt, lang)} .\n")


if __name__ == "__main__":
    main()
