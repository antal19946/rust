import json

input_path = 'data/token_tax_report.jsonl'
output_path = 'data/token_zero_transfer_tax.jsonl'

with open(input_path, 'r') as infile, open(output_path, 'w') as outfile:
    for line in infile:
        try:
            obj = json.loads(line)
            if obj.get('transferTax', None) == 0 and obj.get('buyTax', None) == 0 and obj.get('sellTax', None) == 0 and obj.get('isHoneypot', None) == False:
                outfile.write(json.dumps(obj) + '\n')
        except Exception:
            continue 