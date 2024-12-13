# Usage:
# 
# First run: hey http://127.0.0.1:8080 > raw-output.txt 
# Then run: python ./scripts/parse-hey.py ./raw-output.txt
# Output:
# [
#   {
#     "name": "HTTP RPS",
#     "unit": "req/s",
#     "value": 13334.5075
#   },
#   {
#     "name": "HTTP p95 Latency",
#     "unit": "ms",
#     "value": 4.1000000000000005
#   }
# ]


import json
import re
import sys

def parse_hey_output(file_path):
    with open(file_path, 'r') as f:
        lines = f.readlines()

    rps = None
    lat = None

    for i, line in enumerate(lines):
        line = line.strip()

        if line.startswith("Requests/sec:"):
            parts = line.split()
            if len(parts) >= 2:
                rps = float(parts[1])

        if "Latency distribution:" in line:
            for j in range(i+1, len(lines)):
                if "% in" in lines[j] and "95%" in lines[j]:
                    match = re.search(r'95% in ([0-9.]+) secs', lines[j])
                    if match:
                        lat = float(match.group(1))
                    break

    return rps, lat


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: python parse_hey.py <hey-output-file>")
        sys.exit(1)

    file_path = sys.argv[1]
    rps, latency = parse_hey_output(file_path)

    latency = latency * 1000 if latency is not None else None

    if rps is not None:
        throughput_result = [{
            "name": "HTTP RPS",
            "unit": "req/s",
            "value": rps
        }]
        with open('throughput_results.json', 'w') as f:
            json.dump(throughput_result, f, indent=2)

    if latency is not None:
        latency_result = [{
            "name": "HTTP p95 Latency",
            "unit": "ms",
            "value": latency
        }]
        with open('latency_results.json', 'w') as f:
            json.dump(latency_result, f, indent=2)
