#!/usr/bin/env python3
"""CycloneDX 1.5 SBOM for a Juggernaught release, stdlib only.

Components: every crate linked into the `juggernaught` binary for the target
(normal + build dependencies, resolved from Cargo.lock, with its checksum), plus
every pinned external tool from tools.lock.toml with its per-arch SHA-256.

    python3 scripts/sbom.py [--target aarch64-unknown-linux-gnu] > sbom.cdx.json
"""
import argparse, json, subprocess, sys, tomllib, uuid, datetime, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent

def purl(name, version):
    return f"pkg:cargo/{name}@{version}"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", default=None)
    args = ap.parse_args()
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")]
    if args.target:
        cmd += ["--filter-platform", args.target]
    meta = json.loads(subprocess.check_output(cmd))
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    checksums = {(p["name"], p["version"]): p.get("checksum") for p in lock["package"]}
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(i for i, p in pkgs.items() if p["name"] == "juggernaught")

    # Walk normal + build edges only (dev-dependencies never ship).
    seen, stack = set(), [root]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
                stack.append(d["pkg"])

    def ref(i):
        p = pkgs[i]
        return purl(p["name"], p["version"])

    components, deps = [], []
    for i in sorted(seen - {root}, key=lambda i: (pkgs[i]["name"], pkgs[i]["version"])):
        p = pkgs[i]
        c = {"type": "library", "bom-ref": ref(i), "name": p["name"], "version": p["version"], "purl": ref(i)}
        if p.get("license"):
            c["licenses"] = [{"expression": p["license"]}]
        sha = checksums.get((p["name"], p["version"]))
        if sha:
            c["hashes"] = [{"alg": "SHA-256", "content": sha}]
        if p.get("repository"):
            c["externalReferences"] = [{"type": "vcs", "url": p["repository"]}]
        if p.get("source") is None:
            c["scope"] = "required"
            c["description"] = "workspace crate (this repository)"
        components.append(c)
    for i in sorted(seen, key=ref):
        on = sorted({ref(d["pkg"]) for d in nodes[i]["deps"] if d["pkg"] in seen and any(k["kind"] in (None, "build") for k in d["dep_kinds"])})
        deps.append({"ref": ref(i), "dependsOn": on})

    tools = tomllib.loads((ROOT / "tools.lock.toml").read_text()).get("tool", {})
    for name, t in sorted(tools.items()):
        dist = t.get("dist", {})
        hashes = [{"alg": "SHA-256", "content": v} for k, v in sorted(dist.items()) if k.startswith("sha256")]
        c = {
            "type": "application",
            "bom-ref": f"tool:{name}@{t.get('version', '')}",
            "name": name,
            "version": str(t.get("version", "")),
            "description": "pinned external creative tool, installed on demand and hash-verified",
        }
        if t.get("repo"):
            c["purl"] = f"pkg:github/{t['repo']}@{t.get('version', '')}"
        if t.get("license"):
            c["licenses"] = [{"expression": t["license"]}]
        if hashes:
            c["hashes"] = hashes
        components.append(c)

    root_pkg = pkgs[root]
    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{uuid.uuid4()}",
        "version": 1,
        "metadata": {
            "timestamp": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat(),
            "tools": {"components": [{"type": "application", "name": "juggernaught scripts/sbom.py"}]},
            "component": {"type": "application", "bom-ref": ref(root), "name": "juggernaught", "version": root_pkg["version"], "purl": ref(root),
                          "licenses": [{"expression": root_pkg["license"]}] if root_pkg.get("license") else []},
            "properties": [{"name": "target", "value": args.target or "host"}],
        },
        "components": components,
        "dependencies": deps,
    }
    json.dump(bom, sys.stdout, indent=1)
    sys.stdout.write("\n")
    print(f"sbom: {len(seen) - 1} crates, {len(tools)} pinned tools", file=sys.stderr)

if __name__ == "__main__":
    main()
