"""Check LLVM-exported branch and code-region coverage for connection feedback.

Usage: python scripts/check-connection-feedback-coverage.py path/to/coverage.json
Generate the JSON with cargo llvm-cov --branch --json; see the internal requirements
document for the full command. Include generated closures in their owning function.
"""

import argparse
import json
from pathlib import Path


TARGETS = {
    "connection_failure_message": "core/src/i18n.rs",
    "localize_tunnel_job_feedback": "core/src/runtime/status.rs",
    "relocalize_runtime_status": "core/src/runtime/status.rs",
    "tunnel_job_lifecycle_projection": "core/src/runtime/status.rs",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    report = json.loads(args.report.read_text(encoding="utf-8"))
    results = {name: {"regions": {}, "branches": {}} for name in TARGETS}

    for unit in report["data"]:
        for function in unit["functions"]:
            paths = [name.replace(chr(92), "/") for name in function["filenames"]]
            for name, source in TARGETS.items():
                if name not in function["name"] or not any(p.endswith(source) for p in paths):
                    continue
                # Merge repeated instantiations by source location and function identity.
                for region in function["regions"]:
                    if region[7] != 0:  # Only executable code, excluding gaps/expansions.
                        continue
                    key = (function["name"], paths[region[5]], *region[:4])
                    results[name]["regions"][key] = (
                        results[name]["regions"].get(key, False) or region[4] > 0
                    )
                for branch in function.get("branches", []):
                    key = (function["name"], paths[branch[6]], *branch[:4])
                    previous = results[name]["branches"].get(key, (False, False))
                    results[name]["branches"][key] = (
                        previous[0] or branch[4] > 0,
                        previous[1] or branch[5] > 0,
                    )

    failed = False
    for name, result in results.items():
        for metric in ("regions", "branches"):
            values = list(result[metric].values())
            outcomes = values if metric == "regions" else [v for pair in values for v in pair]
            covered, total = sum(outcomes), len(outcomes)
            percent = 100 * covered / total if total else 0
            print(f"{name}: {metric} {covered}/{total} ({percent:.2f}%)")
            if not total or covered != total:
                failed = True
                for key, value in result[metric].items():
                    if not value or (metric == "branches" and not all(value)):
                        print(f"  uncovered: {key[1]}:{key[2]} ({value})")
    if failed:
        raise SystemExit("Connection feedback requires 100% exported branch and code-region coverage.")


if __name__ == "__main__":
    main()
