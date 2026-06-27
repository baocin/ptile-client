#!/tmp/food-venv/bin/python
"""
Food ingredient embedding utilities using jonny9f/food_embeddings2.

Usage:
  python food_embed.py encode -- "Beef, ground, 80% lean"
  python food_embed.py similar -- "Chicken breast, raw" 5
  python food_embed.py pantry -- /path/to/pantry_list.txt
"""

import sys
import json
import numpy as np
from sentence_transformers import SentenceTransformer

MODEL_NAME = "jonny9f/food_embeddings2"

def get_model():
    return SentenceTransformer(MODEL_NAME)

def encode(ingredients):
    model = get_model()
    return model.encode(ingredients)

def nearest(ingredient, candidates, k=5):
    model = get_model()
    query_emb = model.encode([ingredient])
    cand_embs = model.encode(candidates)
    sims = model.similarity(query_emb, cand_embs).numpy()[0]
    indices = np.argsort(sims)[::-1][:k]
    return [(candidates[i], float(sims[i])) for i in indices]

def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return

    cmd = sys.argv[1]

    if cmd == "encode":
        items = sys.argv[2:]
        if not items:
            items = [sys.stdin.read().strip()]
        embs = encode(items)
        print(json.dumps({"shape": list(embs.shape), "vectors": embs.tolist()}))

    elif cmd == "similar":
        query = sys.argv[2]
        k = int(sys.argv[3]) if len(sys.argv) > 3 else 5
        from pathlib import Path
        candidates_path = Path(__file__).parent / "food_items.txt"
        if candidates_path.exists():
            candidates = candidates_path.read_text().strip().splitlines()
        else:
            candidates = [
                "Tomato, ripe", "Basil, fresh", "Mozzarella cheese",
                "Beef, ground, 80% lean", "Chicken breast, raw",
                "Pasta, spaghetti, dry", "Rice, white, long-grain",
                "Apple, raw", "Banana, raw", "Broccoli, raw",
                "Carrots, raw", "Olive oil, extra virgin",
                "Garlic, raw", "Onions, raw", "Salt, table",
            ]
        results = nearest(query, candidates, k)
        for name, sim in results:
            print(f"{sim:.4f}  {name}")

    elif cmd == "pantry":
        path = sys.argv[2] if len(sys.argv) > 2 else "pantry.txt"
        items = open(path).read().strip().splitlines()
        model = get_model()
        embs = model.encode(items)
        sims = model.similarity(embs, embs).numpy()
        print("Pantry item similarity matrix:")
        for i in range(len(items)):
            for j in range(i+1, len(items)):
                print(f"  {items[i]:30s} <-> {items[j]:30s}: {sims[i][j]:.4f}")

    else:
        print(f"Unknown command: {cmd}")

if __name__ == "__main__":
    main()
