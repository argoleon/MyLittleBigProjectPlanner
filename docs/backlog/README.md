# MVP backlog

Source files of the MVP backlog published to GitHub: user stories and their tech stories, grouped by work package (section 20 of the [specification](../en/specification.md)).

- Issues: <https://github.com/argoleon/MyLittleBigProjectPlanner/issues>
- Project board: <https://github.com/users/argoleon/projects/1>

## How it is organized on GitHub

| Backlog | GitHub |
| --- | --- |
| Work package (L01…L17) | Milestone `L01 Technical foundation`, … |
| User story (`L05-US03`) | Issue labelled `user-story` |
| Tech story (`L05-US03-T02`) | Sub-issue of its user story, labelled `tech-story` and `layer:*` |
| Level | Label `essential` or `important` |
| Dependency between user stories | "Blocked by" relationship |

Each issue body starts with a hidden marker `<!-- backlog-id: … -->` that links it to its entry in `packages/`.

## Files

- `packages/L*.json`: one file per work package (user stories, acceptance criteria, tech stories).
- `create_backlog.py`: creates labels, milestones, issues, sub-issues, dependencies and project items with the `gh` CLI.
- `state.json`: mapping from backlog IDs to the GitHub issues already created. Keep it committed: it is what makes the script safe to re-run.

## Adding to the backlog

GitHub is the place where the backlog lives day to day: edit, split or close issues there. The JSON files are the initial import, not a mirror kept in sync.

To add a batch of new stories through the script:

1. Add the stories to the matching `packages/L*.json` file, with new unique IDs.
2. Preview: `python3 docs/backlog/create_backlog.py --dry-run`. Only the new items should be listed.
3. Create: `python3 docs/backlog/create_backlog.py --project "MLBPP MVP"`.
4. Commit the updated `state.json` with the JSON files, on a feature branch.

The script never edits or deletes existing issues. It needs `gh` authenticated with the `repo` and `project` scopes (`gh auth login --scopes project`).
