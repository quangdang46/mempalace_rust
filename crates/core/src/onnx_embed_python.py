#!/usr/bin/env python3
import sys
import json

persistent_mode = len(sys.argv) > 1 and sys.argv[1] == "--persistent"

from chromadb.utils.embedding_functions.onnx_mini_lm_l6_v2 import ONNXMiniLM_L6_V2

model = ONNXMiniLM_L6_V2()

if persistent_mode:
    while True:
        line = sys.stdin.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
        if line == "PERSISTENT":
            print("OK", flush=True)
            continue
        if line == "QUIT":
            break
        try:
            texts = json.loads(line)
            if isinstance(texts, str):
                texts = [texts]
            embeddings = model(input=texts)
            for emb in embeddings:
                print(json.dumps(emb.tolist()))
                sys.stdout.flush()
            print("DONE", flush=True)
        except Exception as e:
            print(f"ERROR: {e}", file=sys.stderr)
            sys.stderr.flush()
else:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            texts = json.loads(line)
            if isinstance(texts, str):
                texts = [texts]
            embeddings = model(input=texts)
            for emb in embeddings:
                print(json.dumps(emb.tolist()))
                sys.stdout.flush()
        except Exception as e:
            print(f"ERROR: {e}", file=sys.stderr)
            sys.stderr.flush()
