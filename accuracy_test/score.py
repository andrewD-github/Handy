#!/usr/bin/env python3
"""Score Handy transcription accuracy (WER/CER) against a reference passage.

Pulls the most recent transcription from the app's history.db and compares it to
reference.txt. Run after reading the reference passage aloud into Handy.

Usage:
    python score.py                 # score the single latest transcription
    python score.py --list 10       # show the 10 most recent transcriptions, pick one
    python score.py --id 42         # score a specific history row id
    python score.py --text "..."    # score arbitrary text instead of the DB
"""
import argparse
import os
import re
import sqlite3
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_DB = os.path.join(
    HERE, "..", "src-tauri", "target", "release", "Data", "history.db"
)
DEFAULT_REF = os.path.join(HERE, "reference.txt")


def normalize(text):
    """Lowercase, drop punctuation, collapse whitespace -> list of word tokens."""
    text = text.lower()
    text = re.sub(r"[^\w\s]", " ", text)        # strip punctuation
    text = re.sub(r"\s+", " ", text).strip()
    return text.split()


def edit_align(ref, hyp):
    """Levenshtein over token lists; returns (S, D, I, H, ops) with backtrace."""
    n, m = len(ref), len(hyp)
    d = [[0] * (m + 1) for _ in range(n + 1)]
    bt = [[None] * (m + 1) for _ in range(n + 1)]
    for i in range(1, n + 1):
        d[i][0] = i
        bt[i][0] = "D"
    for j in range(1, m + 1):
        d[0][j] = j
        bt[0][j] = "I"
    for i in range(1, n + 1):
        for j in range(1, m + 1):
            if ref[i - 1] == hyp[j - 1]:
                d[i][j] = d[i - 1][j - 1]
                bt[i][j] = "H"
            else:
                sub, dele, ins = d[i - 1][j - 1], d[i - 1][j], d[i][j - 1]
                best = min(sub, dele, ins)
                d[i][j] = best + 1
                bt[i][j] = "S" if best == sub else ("D" if best == dele else "I")
    # backtrace
    i, j, ops = n, m, []
    while i > 0 or j > 0:
        op = bt[i][j]
        if op == "H":
            ops.append(("ok", ref[i - 1], hyp[j - 1])); i -= 1; j -= 1
        elif op == "S":
            ops.append(("sub", ref[i - 1], hyp[j - 1])); i -= 1; j -= 1
        elif op == "D":
            ops.append(("del", ref[i - 1], "")); i -= 1
        else:
            ops.append(("ins", "", hyp[j - 1])); j -= 1
    ops.reverse()
    S = sum(1 for o in ops if o[0] == "sub")
    D = sum(1 for o in ops if o[0] == "del")
    I = sum(1 for o in ops if o[0] == "ins")
    H = sum(1 for o in ops if o[0] == "ok")
    return S, D, I, H, ops


def cer(ref, hyp):
    r, h = " ".join(ref), " ".join(hyp)
    S, D, I, _, _ = edit_align(list(r), list(h))
    return (S + D + I) / max(len(r), 1)


def get_transcripts(db_path, limit):
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    rows = con.execute(
        "SELECT id, timestamp, transcription_text FROM transcription_history "
        "ORDER BY timestamp DESC LIMIT ?",
        (limit,),
    ).fetchall()
    con.close()
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", default=DEFAULT_DB)
    ap.add_argument("--ref", default=DEFAULT_REF)
    ap.add_argument("--id", type=int, help="score a specific history row id")
    ap.add_argument("--list", type=int, metavar="N", help="list N latest and exit")
    ap.add_argument("--text", help="score this literal text instead of the DB")
    args = ap.parse_args()

    with open(args.ref, encoding="utf-8") as f:
        ref_raw = f.read().strip()
    ref = normalize(ref_raw)

    if args.list:
        for r in get_transcripts(args.db, args.list):
            preview = r["transcription_text"].replace("\n", " ")[:90]
            print(f'  id={r["id"]:>4}  ts={r["timestamp"]}  "{preview}"')
        return

    if args.text is not None:
        hyp_raw = args.text
    else:
        rows = get_transcripts(args.db, 50)
        if not rows:
            sys.exit("No transcriptions in history.db yet — read the passage first.")
        if args.id is not None:
            match = [r for r in rows if r["id"] == args.id]
            if not match:
                sys.exit(f"id {args.id} not found in latest 50 rows.")
            hyp_raw = match[0]["transcription_text"]
        else:
            hyp_raw = rows[0]["transcription_text"]

    hyp = normalize(hyp_raw)
    S, D, I, H, ops = edit_align(ref, hyp)
    N = len(ref)
    wer = (S + D + I) / max(N, 1)
    acc = 100 * H / max(N, 1)

    print("=" * 70)
    print("REFERENCE :", ref_raw)
    print("-" * 70)
    print("TRANSCRIPT:", hyp_raw.strip())
    print("=" * 70)
    print(f"Words in reference : {N}")
    print(f"Correct            : {H}")
    print(f"Substitutions      : {S}")
    print(f"Deletions (missed) : {D}")
    print(f"Insertions (extra) : {I}")
    print("-" * 70)
    print(f"WER (word error rate)   : {wer*100:5.1f}%")
    print(f"Word accuracy           : {acc:5.1f}%")
    print(f"CER (char error rate)   : {cer(ref, hyp)*100:5.1f}%")
    print("=" * 70)
    print("Errors (ref -> got):")
    any_err = False
    for kind, r, h in ops:
        if kind == "sub":
            print(f"  ~ {r!r:20} -> {h!r}"); any_err = True
        elif kind == "del":
            print(f"  - {r!r:20} (missed)"); any_err = True
        elif kind == "ins":
            print(f"  + {'':20}    {h!r} (extra)"); any_err = True
    if not any_err:
        print("  none — perfect match")
    print("\nNote: WER ignores capitalization & punctuation (standard practice).")
    print("Numbers count as errors if spoken/written differently (e.g. 150 vs one hundred fifty).")


if __name__ == "__main__":
    main()
