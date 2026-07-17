#!/usr/bin/env python3
"""Track a GitHub issue-backed devlog milestone."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, Sequence


DEVLOG_LABEL = "devlog"
DEVLOG_NOTE_MARKER = "<!-- devlog-entry -->"
PLANNED_HEADING = "Planned work"


class DevlogError(RuntimeError):
    pass


def run_gh(arguments: Sequence[str]) -> str:
    result = subprocess.run(
        ["gh", *arguments],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "unknown gh error"
        raise DevlogError(detail)
    return result.stdout


def run_gh_json(arguments: Sequence[str]) -> Any:
    output = run_gh(arguments)
    try:
        return json.loads(output)
    except json.JSONDecodeError as error:
        raise DevlogError(f"gh returned invalid JSON: {error}") from error


def parse_timestamp(value: str) -> datetime:
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise DevlogError(f"Invalid GitHub timestamp: {value}") from error


def issue_labels(issue: dict[str, Any]) -> set[str]:
    return {label["name"] for label in issue.get("labels", [])}


def active_milestone() -> dict[str, Any] | None:
    issues = run_gh_json(
        [
            "issue",
            "list",
            "--state",
            "open",
            "--label",
            DEVLOG_LABEL,
            "--limit",
            "100",
            "--json",
            "number,title,body,state,createdAt,closedAt,url,labels",
        ]
    )
    if len(issues) > 1:
        numbers = ", ".join(f"#{issue['number']}" for issue in issues)
        raise DevlogError(f"Multiple active devlog milestones found: {numbers}")
    return issues[0] if issues else None


def milestone_by_number(number: int) -> dict[str, Any]:
    issue = run_gh_json(
        [
            "issue",
            "view",
            str(number),
            "--json",
            "number,title,body,state,createdAt,closedAt,url,labels",
        ]
    )
    if DEVLOG_LABEL not in issue_labels(issue):
        raise DevlogError(f"Issue #{number} is not labeled {DEVLOG_LABEL!r}")
    return issue


def selected_milestone(number: int | None) -> dict[str, Any]:
    if number is not None:
        return milestone_by_number(number)
    issue = active_milestone()
    if issue is None:
        raise DevlogError("No active devlog milestone found")
    return issue


def planned_issue_numbers(body: str) -> list[int]:
    heading_pattern = re.compile(
        rf"(?ims)^##\s+{re.escape(PLANNED_HEADING)}\s*$\n(?P<section>.*?)(?=^##\s+|\Z)"
    )
    match = heading_pattern.search(body)
    if match is None:
        return []
    seen: set[int] = set()
    numbers: list[int] = []
    for value in re.findall(r"(?<![\w/])#(\d+)\b", match.group("section")):
        number = int(value)
        if number not in seen:
            seen.add(number)
            numbers.append(number)
    return numbers


def issue_details(number: int) -> dict[str, Any]:
    return run_gh_json(
        [
            "issue",
            "view",
            str(number),
            "--json",
            "number,title,state,closedAt,url,labels,comments",
        ]
    )


def devlog_notes(issue: dict[str, Any]) -> list[str]:
    notes: list[str] = []
    for comment in issue.get("comments", []):
        body = comment.get("body", "")
        if DEVLOG_NOTE_MARKER not in body:
            continue
        note = body.split(DEVLOG_NOTE_MARKER, maxsplit=1)[1].strip()
        note = re.sub(r"^###\s+Devlog note\s*", "", note, flags=re.IGNORECASE).strip()
        notes.append(note)
    return notes


def closed_issues_in_window(start: datetime, end: datetime) -> list[dict[str, Any]]:
    search = f"closed:{start.date().isoformat()}..{end.date().isoformat()}"
    issues = run_gh_json(
        [
            "issue",
            "list",
            "--state",
            "closed",
            "--limit",
            "1000",
            "--search",
            search,
            "--json",
            "number,title,state,closedAt,url,labels",
        ]
    )
    result = []
    for issue in issues:
        closed_at_value = issue.get("closedAt")
        if closed_at_value is None or DEVLOG_LABEL in issue_labels(issue):
            continue
        closed_at = parse_timestamp(closed_at_value)
        if start <= closed_at <= end:
            result.append(issue_details(issue["number"]))
    return sorted(result, key=lambda issue: (issue.get("closedAt") or "", issue["number"]))


@dataclass
class Inventory:
    milestone: dict[str, Any]
    start: datetime
    end: datetime
    planned: list[dict[str, Any]]
    incidental: list[dict[str, Any]]

    @property
    def open_planned(self) -> list[dict[str, Any]]:
        return [issue for issue in self.planned if issue["state"] != "CLOSED"]


def build_inventory(milestone: dict[str, Any]) -> Inventory:
    start = parse_timestamp(milestone["createdAt"])
    closed_at_value = milestone.get("closedAt")
    end = parse_timestamp(closed_at_value) if closed_at_value else datetime.now(timezone.utc)
    planned_numbers = planned_issue_numbers(milestone.get("body", ""))
    planned = [issue_details(number) for number in planned_numbers]
    planned_set = set(planned_numbers)
    incidental = [
        issue
        for issue in closed_issues_in_window(start, end)
        if issue["number"] not in planned_set
    ]
    return Inventory(milestone, start, end, planned, incidental)


def issue_markdown(issue: dict[str, Any]) -> list[str]:
    state = issue["state"].lower()
    lines = [f"### [#{issue['number']} — {issue['title']}]({issue['url']})", "", f"State: {state}."]
    notes = devlog_notes(issue)
    if notes:
        for note in notes:
            lines.extend(["", note])
    elif issue["state"] == "CLOSED":
        lines.extend(["", "_Missing structured devlog note._"])
    return lines


def render_inventory(inventory: Inventory) -> str:
    milestone = inventory.milestone
    lines = [
        f"# Devlog inventory: {milestone['title']}",
        "",
        f"Milestone: [#{milestone['number']}]({milestone['url']})",
        f"Window: {inventory.start.isoformat()} to {inventory.end.isoformat()}",
        "",
        "## Completed planned work",
    ]
    completed_planned = [issue for issue in inventory.planned if issue["state"] == "CLOSED"]
    if completed_planned:
        for issue in completed_planned:
            lines.extend(["", *issue_markdown(issue)])
    else:
        lines.extend(["", "_None._"])

    lines.extend(["", "## Open planned work"])
    if inventory.open_planned:
        for issue in inventory.open_planned:
            lines.extend(["", *issue_markdown(issue)])
    else:
        lines.extend(["", "_None._"])

    lines.extend(["", "## Incidental work"])
    if inventory.incidental:
        for issue in inventory.incidental:
            lines.extend(["", *issue_markdown(issue)])
    else:
        lines.extend(["", "_None._"])

    missing_notes = [
        issue
        for issue in [*completed_planned, *inventory.incidental]
        if not devlog_notes(issue)
    ]
    lines.extend(["", "## Capture gaps"])
    if missing_notes:
        links = ", ".join(f"[#{issue['number']}]({issue['url']})" for issue in missing_notes)
        lines.extend(["", f"Missing structured devlog notes: {links}."])
    else:
        lines.extend(["", "_None._"])
    return "\n".join(lines).rstrip() + "\n"


def command_active(_: argparse.Namespace) -> None:
    issue = active_milestone()
    if issue is None:
        print("No active devlog milestone.")
        return
    planned = planned_issue_numbers(issue.get("body", ""))
    print(
        json.dumps(
            {
                "number": issue["number"],
                "title": issue["title"],
                "url": issue["url"],
                "createdAt": issue["createdAt"],
                "plannedIssues": planned,
            },
            indent=2,
        )
    )


def command_start(arguments: argparse.Namespace) -> None:
    existing = active_milestone()
    if existing is not None:
        raise DevlogError(
            f"Devlog milestone #{existing['number']} is already active: {existing['url']}"
        )
    planned_lines = (
        "\n".join(f"- [ ] #{number}" for number in arguments.planned)
        if arguments.planned
        else "_No planned issues yet._"
    )
    body = (
        "<!-- devlog-milestone -->\n"
        "## Goal\n\n"
        f"{arguments.goal.strip()}\n\n"
        f"## {PLANNED_HEADING}\n\n"
        f"{planned_lines}\n\n"
        "## Production notes\n\n"
        "Record only milestone-level narrative or footage notes here. "
        "Issue-specific notes belong on their work issues.\n"
    )
    output = run_gh(
        [
            "issue",
            "create",
            "--title",
            f"Devlog: {arguments.title.strip()}",
            "--label",
            DEVLOG_LABEL,
            "--body",
            body,
        ]
    )
    print(output.strip())


def command_note(arguments: argparse.Namespace) -> None:
    milestone = active_milestone()
    if milestone is None:
        raise DevlogError("No active devlog milestone; no devlog note was added")
    if arguments.issue == milestone["number"]:
        raise DevlogError("Add work notes to a work issue, not the milestone issue")
    fields = [
        ("Viewer impact", arguments.impact),
        ("Before", arguments.before),
        ("After", arguments.after),
        ("Evidence", arguments.evidence),
        ("Visual", arguments.visual),
    ]
    lines = [DEVLOG_NOTE_MARKER, "### Devlog note", ""]
    lines.extend(f"- **{name}:** {value.strip()}" for name, value in fields if value)
    body = "\n".join(lines)
    output = run_gh(["issue", "comment", str(arguments.issue), "--body", body])
    print(output.strip())


def command_report(arguments: argparse.Namespace) -> None:
    inventory = build_inventory(selected_milestone(arguments.issue))
    print(render_inventory(inventory), end="")


def command_finish(arguments: argparse.Namespace) -> None:
    milestone = selected_milestone(arguments.issue)
    if milestone["state"] != "OPEN":
        raise DevlogError(f"Milestone #{milestone['number']} is already closed")
    inventory = build_inventory(milestone)
    if inventory.open_planned and not arguments.allow_open_planned:
        numbers = ", ".join(f"#{issue['number']}" for issue in inventory.open_planned)
        raise DevlogError(f"Planned issues are still open: {numbers}")
    report = render_inventory(inventory)
    run_gh(["issue", "comment", str(milestone["number"]), "--body", report])
    output = run_gh(
        [
            "issue",
            "close",
            str(milestone["number"]),
            "--comment",
            "Devlog inventory captured; closing the reporting window.",
        ]
    )
    print(report, end="")
    if output.strip():
        print(output.strip(), file=sys.stderr)


def parser() -> argparse.ArgumentParser:
    argument_parser = argparse.ArgumentParser(
        description="Track a GitHub issue-backed YouTube devlog milestone."
    )
    subparsers = argument_parser.add_subparsers(dest="command", required=True)

    active_parser = subparsers.add_parser("active", help="Show the active milestone")
    active_parser.set_defaults(handler=command_active)

    start_parser = subparsers.add_parser("start", help="Create a devlog milestone")
    start_parser.add_argument("--title", required=True)
    start_parser.add_argument("--goal", required=True)
    start_parser.add_argument("--planned", nargs="*", type=int, default=[])
    start_parser.set_defaults(handler=command_start)

    note_parser = subparsers.add_parser("note", help="Comment a structured note on a work issue")
    note_parser.add_argument("--issue", type=int, required=True)
    note_parser.add_argument("--impact", required=True)
    note_parser.add_argument("--before")
    note_parser.add_argument("--after")
    note_parser.add_argument("--evidence", required=True)
    note_parser.add_argument("--visual")
    note_parser.set_defaults(handler=command_note)

    report_parser = subparsers.add_parser("report", help="Render a devlog inventory")
    report_parser.add_argument("--issue", type=int)
    report_parser.set_defaults(handler=command_report)

    finish_parser = subparsers.add_parser("finish", help="Post the inventory and close the milestone")
    finish_parser.add_argument("--issue", type=int)
    finish_parser.add_argument("--allow-open-planned", action="store_true")
    finish_parser.set_defaults(handler=command_finish)
    return argument_parser


def main() -> int:
    arguments = parser().parse_args()
    try:
        arguments.handler(arguments)
    except DevlogError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
