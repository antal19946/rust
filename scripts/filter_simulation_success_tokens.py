import json

input_path = 'data/token_tax_report.jsonl'
output_path = 'data/token_tax_report_sim_success.jsonl'

with open(input_path, 'r') as infile, open(output_path, 'w') as outfile:
    for line in infile:
        try:
            obj = json.loads(line)
            if obj.get('simulationSuccess') is True:
                outfile.write(json.dumps(obj) + '\n')
        except Exception as e:
            # Optionally log or print error, skip bad lines
            continue 