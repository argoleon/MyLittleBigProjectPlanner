#!/usr/bin/env python3
"""Create the MVP backlog on GitHub from packages/L*.json.

Idempotent: every issue body carries a hidden marker `<!-- backlog-id: X -->`;
existing issues are found by that marker and never recreated. Progress is also
saved in state.json next to this script.

Usage:
  create_backlog.py --dry-run          # print what would be created
  create_backlog.py                    # create labels, milestones, issues, sub-issues, deps
  create_backlog.py --project "Title"  # also create/fill a GitHub Project (v2)
"""
import argparse
import glob
import json
import os
import subprocess
import sys
import time

REPO = "argoleon/MyLittleBigProjectPlanner"
OWNER = REPO.split("/")[0]
HERE = os.path.dirname(os.path.abspath(__file__))
STATE_FILE = os.path.join(HERE, "state.json")
SPEC_URL = f"https://github.com/{REPO}/blob/develop/docs/en/specification.md"
REPO_BLOB = f"https://github.com/{REPO}/blob/develop/"

LABELS = {
    "user-story": ("1D76DB", "User story: a slice of user value"),
    "tech-story": ("5319E7", "Technical story attached to a user story"),
    "essential": ("B60205", "Essential: gates the MVP production release"),
    "important": ("FBCA04", "Important: may slip just after the MVP"),
    "layer:database": ("C5DEF5", "Schema, migrations, RLS"),
    "layer:domain": ("C5DEF5", "Business rules in crates/domain"),
    "layer:backend": ("C5DEF5", "API, infra crates"),
    "layer:worker": ("C5DEF5", "Background jobs"),
    "layer:frontend": ("C5DEF5", "Angular app, portal, ui library"),
    "layer:infra": ("C5DEF5", "CI, Docker Compose, operations"),
    "layer:e2e": ("C5DEF5", "Playwright end-to-end tests"),
}


def gh(args, payload=None, check=True):
    cmd = ["gh"] + args
    res = subprocess.run(cmd, input=json.dumps(payload) if payload is not None else None,
                         capture_output=True, text=True)
    if check and res.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}\n{res.stderr.strip()}")
    return res


def api(method, path, payload=None, check=True):
    args = ["api", "-X", method, path, "-H", "Accept: application/vnd.github+json"]
    if payload is not None:
        args += ["--input", "-"]
    res = gh(args, payload, check)
    return json.loads(res.stdout) if res.stdout.strip() else None


def load_state():
    if os.path.exists(STATE_FILE):
        with open(STATE_FILE) as f:
            return json.load(f)
    return {"issues": {}, "milestones": {}, "project_items": []}


def save_state(state):
    with open(STATE_FILE, "w") as f:
        json.dump(state, f, indent=1)


def load_backlog():
    lots = []
    for path in sorted(glob.glob(os.path.join(HERE, "packages", "L*.json"))):
        with open(path) as f:
            lots.append(json.load(f))
    return lots


def bullets(items):
    return "\n".join(f"- {i}" for i in items) if items else "_None_"


def spec_refs(refs):
    return ", ".join(f"[{r}]({SPEC_URL})" for r in refs) if refs else "_None_"


def mockup_refs(refs):
    return ", ".join(f"[{os.path.basename(m)}]({REPO_BLOB}{m})" for m in refs) if refs else "_None_"


def story_body(lot, s):
    return f"""<!-- backlog-id: {s['id']} -->
**{s['id']}** · Work package **{lot['lot']} {lot['title']}** · Level: **{s['level']}**

## User story
As **{s['as_a']}**, I want **{s['i_want']}**, so that **{s['so_that']}**.

## Acceptance criteria
{bullets(s['acceptance'])}

## References
- Specification: {spec_refs(s.get('spec'))}
- Mockups: {mockup_refs(s.get('mockups'))}
- Depends on: {", ".join(s.get('depends_on') or []) or "_None_"}

## Definition of done
- All tech stories (sub-issues) closed.
- Unit and integration tests written and passing in CI (mandatory, see CLAUDE.md).
- Developed on a `feature/{lot['lot']}-…` branch, reviewed, merged into `develop`.
"""


def tech_body(lot, s, t):
    return f"""<!-- backlog-id: {t['id']} -->
**{t['id']}** · User story **{s['id']} {s['title']}** · Layer: **{t['layer']}**

## Description
{t['description']}

## Done when
{bullets(t['done_when'])}
- Unit and integration tests written and passing in CI (mandatory, see CLAUDE.md).
"""


def find_existing(marker_id):
    q = f'repo:{REPO} in:body "backlog-id: {marker_id}"'
    res = gh(["api", "-X", "GET", "search/issues", "-f", f"q={q}"], check=False)
    if res.returncode != 0:
        return None
    for item in json.loads(res.stdout).get("items", []):
        if f"backlog-id: {marker_id} -->" in (item.get("body") or ""):
            return {"number": item["number"], "id": item["id"], "node_id": item["node_id"]}
    return None


def ensure_labels(dry):
    existing = {l["name"] for l in api("GET", f"repos/{REPO}/labels?per_page=100")} if not dry else set()
    for name, (color, desc) in LABELS.items():
        if name in existing:
            continue
        print(f"label  + {name}")
        if not dry:
            api("POST", f"repos/{REPO}/labels", {"name": name, "color": color, "description": desc})


def ensure_milestones(lots, state, dry):
    existing = {}
    if not dry:
        for m in api("GET", f"repos/{REPO}/milestones?state=all&per_page=100"):
            existing[m["title"]] = m["number"]
    for lot in lots:
        title = f"{lot['lot']} {lot['title']}"
        if title in existing:
            state["milestones"][lot["lot"]] = existing[title]
            continue
        print(f"milestone + {title}")
        if not dry:
            desc = f"[{lot['level']}] {lot['summary']} Depends on: {', '.join(lot.get('depends_on') or []) or 'none'}."
            m = api("POST", f"repos/{REPO}/milestones", {"title": title, "description": desc[:1000]})
            state["milestones"][lot["lot"]] = m["number"]
            save_state(state)


def ensure_issue(state, key, title, body, labels, milestone, dry):
    if key in state["issues"]:
        return state["issues"][key]
    found = None if dry else find_existing(key)
    if found:
        state["issues"][key] = found
        save_state(state)
        return found
    print(f"issue  + {key}  {title}")
    if dry:
        return None
    issue = api("POST", f"repos/{REPO}/issues",
                {"title": title, "body": body, "labels": labels, "milestone": milestone})
    rec = {"number": issue["number"], "id": issue["id"], "node_id": issue["node_id"]}
    state["issues"][key] = rec
    save_state(state)
    time.sleep(1.2)  # stay under GitHub's content-creation secondary rate limit
    return rec


def link_sub_issue(state, parent, child, key, dry):
    done = state.setdefault("sub_links", [])
    if dry or key in done:
        return
    res = gh(["api", "-X", "POST", f"repos/{REPO}/issues/{parent['number']}/sub_issues",
              "-H", "Accept: application/vnd.github+json", "--input", "-"],
             {"sub_issue_id": child["id"]}, check=False)
    if res.returncode != 0 and "already" not in res.stderr.lower():
        print(f"  ! sub-issue {key} not linked: {res.stderr.strip()[:120]}")
        return
    done.append(key)
    save_state(state)


def link_dependency(state, blocked, blocker, key):
    done = state.setdefault("dep_links", [])
    if key in done:
        return
    res = gh(["api", "-X", "POST", f"repos/{REPO}/issues/{blocked['number']}/dependencies/blocked_by",
              "-H", "Accept: application/vnd.github+json", "--input", "-"],
             {"issue_id": blocker["id"]}, check=False)
    if res.returncode != 0 and "already" not in res.stderr.lower():
        print(f"  ! dependency {key} not linked: {res.stderr.strip()[:120]}")
    done.append(key)
    save_state(state)


def ensure_project(state, title, dry):
    if dry:
        print(f"project + {title} (all issues added)")
        return
    if "project_number" not in state:
        res = gh(["project", "list", "--owner", OWNER, "--format", "json", "--limit", "100"])
        for p in json.loads(res.stdout)["projects"]:
            if p["title"] == title:
                state["project_number"] = p["number"]
        if "project_number" not in state:
            res = gh(["project", "create", "--owner", OWNER, "--title", title, "--format", "json"])
            state["project_number"] = json.loads(res.stdout)["number"]
        save_state(state)
        gh(["project", "link", str(state["project_number"]), "--owner", OWNER, "--repo", REPO], check=False)
    for key, rec in state["issues"].items():
        if key in state["project_items"]:
            continue
        url = f"https://github.com/{REPO}/issues/{rec['number']}"
        res = gh(["project", "item-add", str(state["project_number"]), "--owner", OWNER, "--url", url], check=False)
        if res.returncode != 0 and "already exists" not in res.stderr:
            raise RuntimeError(res.stderr.strip())
        state["project_items"].append(key)
        save_state(state)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--project", help="GitHub Project (v2) title to create and fill")
    ap.add_argument("--lots", help="comma-separated subset, e.g. L01,L02")
    a = ap.parse_args()

    lots = load_backlog()
    if a.lots:
        wanted = set(a.lots.split(","))
        lots = [l for l in lots if l["lot"] in wanted]
    state = load_state()
    n_us = sum(len(l["stories"]) for l in lots)
    n_ts = sum(len(s["tech"]) for l in lots for s in l["stories"])
    print(f"{len(lots)} work packages, {n_us} user stories, {n_ts} tech stories")

    ensure_labels(a.dry_run)
    ensure_milestones(lots, state, a.dry_run)

    for lot in lots:
        ms = state["milestones"].get(lot["lot"])
        for s in lot["stories"]:
            us = ensure_issue(state, s["id"], s["title"], story_body(lot, s),
                              ["user-story", s["level"]], ms, a.dry_run)
            for t in s["tech"]:
                ts = ensure_issue(state, t["id"], t["title"], tech_body(lot, s, t),
                                  ["tech-story", s["level"], f"layer:{t['layer']}"], ms, a.dry_run)
                link_sub_issue(state, us, ts, f"{s['id']}>{t['id']}", a.dry_run)

    if not a.dry_run:
        for lot in lots:
            for s in lot["stories"]:
                for dep in s.get("depends_on") or []:
                    if dep in state["issues"]:
                        link_dependency(state, state["issues"][s["id"]], state["issues"][dep], f"{s['id']}<{dep}")

    if a.project:
        ensure_project(state, a.project, a.dry_run)
    print("done")


if __name__ == "__main__":
    sys.exit(main())
