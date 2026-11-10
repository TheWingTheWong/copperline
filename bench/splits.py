"""Seal held-out boards before any router code exists.

Reads PCBWorld's D3 split file (bench/d3_pcbworld.json) and writes bench/splits.json:

  dev-easy     easy/train  (99)   used during development
  dev-medium   medium/train minus sealed-B (200)
  dev-hard     hard/train   minus sealed-C (200)
  test-a/b/c   PCBWorld's held-out test splits (99/10/10)
  sealed-b/c   86 medium-train and 92 hard-train boards drawn with seed 20261110

Only the dev-* splits may be routed before the frozen release commit.
"""
import json
import random
from pathlib import Path

HERE = Path(__file__).resolve().parent
SEED = 20261110


def main():
    d3 = json.loads((HERE / "d3_pcbworld.json").read_text())
    rng = random.Random(SEED)
    med = sorted(d3["medium"]["train"])
    hard = sorted(d3["hard"]["train"])
    sealed_b = sorted(rng.sample(med, 86))
    sealed_c = sorted(rng.sample(hard, 92))
    out = {
        "seed": SEED,
        "dev-easy": sorted(d3["easy"]["train"]),
        "dev-medium": [n for n in med if n not in set(sealed_b)],
        "dev-hard": [n for n in hard if n not in set(sealed_c)],
        "test-a": sorted(d3["easy"]["test"]),
        "test-b": sorted(d3["medium"]["test"]),
        "test-c": sorted(d3["hard"]["test"]),
        "sealed-b": sealed_b,
        "sealed-c": sealed_c,
    }
    (HERE / "splits.json").write_text(json.dumps(out, indent=1) + "\n")
    print({k: len(v) for k, v in out.items() if k != "seed"})


if __name__ == "__main__":
    main()
