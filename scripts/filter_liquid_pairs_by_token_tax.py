import json

# Step 1: Build set of successful token addresses (field is 'token')
success_tokens = set()
with open('data/token_zero_transfer_tax.jsonl', 'r') as infile:
    for line in infile:
        try:
            obj = json.loads(line)
            token_addr = obj.get('token')  # Correct field is 'token'
            if token_addr:
                success_tokens.add(token_addr.lower())  # Lowercase for safety
        except Exception:
            continue

# Step 2: Filter pools where both token0 and token1 are in the set
with open('data/liquid_pairs_v2_new.jsonl', 'r') as infile, \
     open('data/liquid_pairs_v2_accurate_taxed.jsonl', 'w') as outfile:
    for line in infile:
        try:
            obj = json.loads(line)
            token0 = obj.get('token0')
            token1 = obj.get('token1')
            if token0 and token1 and token0.lower() in success_tokens and token1.lower() in success_tokens:
                outfile.write(json.dumps(obj) + '\n')
        except Exception:
            continue

# NOTE: If your token field names are different, edit 'tokenAddress', 'token0', 'token1' above. 