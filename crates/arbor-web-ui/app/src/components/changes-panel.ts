import { openCreateWorktreeModal } from "./create-worktree-modal";
import { el, changeKindInfo, formatAge } from "../utils";
import {
  state,
  subscribe,
  refreshIssues,
  selectedIssueRepoRoot,
  setRightPanelTab,
} from "../state";

export function createChangesPanel(): HTMLElement {
  const panel = el("div", "changes-panel");
  panel.setAttribute("data-testid", "changes-panel");

  function render(): void {
    panel.replaceChildren();

    panel.append(renderHeader());

    if (state.rightPanelTab === "issues") {
      panel.append(renderIssuesContent());
      return;
    }

    panel.append(renderChangesContent());
  }

  subscribe(render);
  render();
  return panel;
}

function renderHeader(): HTMLElement {
  const header = el("div", "changes-header");
  const tabs = el("div", "changes-tabs");
  tabs.append(
    buildTabButton("Changes", "changes", state.changedFiles.length),
    buildTabButton("Issues", "issues", state.issues.length),
  );

  const actions = el("div", "changes-actions");
  if (state.rightPanelTab === "issues") {
    const refreshButton = el("button", "changes-action-btn", "Refresh");
    refreshButton.type = "button";
    refreshButton.addEventListener("click", () => {
      refreshIssues(selectedIssueRepoRoot(), true);
    });
    actions.append(refreshButton);
  }

  header.append(tabs, actions);
  return header;
}

function renderChangesContent(): HTMLElement {
  if (state.selectedWorktreePath === null) {
    return el("div", "changes-empty", "Select a worktree");
  }

  if (state.changedFiles.length === 0) {
    return el("div", "changes-empty", "No changes");
  }

  const list = el("ul", "changes-list");
  for (const file of state.changedFiles) {
    const info = changeKindInfo(file.kind);
    const item = el("li", "changes-item");

    const statusBadge = el("span", "changes-status");
    statusBadge.textContent = info.code;
    statusBadge.style.color = info.color;

    const pathEl = el("span", "changes-path", file.path);
    pathEl.title = file.path;

    const stats = el("span", "changes-stats");
    if (file.additions > 0) {
      stats.append(el("span", "changes-additions", `+${file.additions}`));
    }
    if (file.deletions > 0) {
      stats.append(el("span", "changes-deletions", `-${file.deletions}`));
    }

    item.append(statusBadge, pathEl, stats);
    list.append(item);
  }
  return list;
}

function renderIssuesContent(): HTMLElement {
  const repoRoot = selectedIssueRepoRoot();
  if (repoRoot === null) {
    return el("div", "changes-empty", "Select a repository");
  }

  if (state.issuesLoading) {
    return el("div", "changes-empty", "Loading issues…");
  }

  if (state.issuesError !== null) {
    const error = el("div", "changes-empty changes-empty-error", state.issuesError);
    return error;
  }

  if (state.issuesNotice !== null) {
    return el("div", "changes-empty", state.issuesNotice);
  }

  const wrapper = el("div", "issues-panel");
  const source = el("div", "issues-source");
  const sourceLabel = state.issueSource !== null
    ? `${state.issueSource.label} · ${state.issueSource.repository}`
    : repoRoot;
  source.append(el("span", "issues-source-label", sourceLabel));
  if (state.issueSource !== null && state.issueSource.url !== null) {
    const link = document.createElement("a");
    link.className = "issues-source-link";
    link.href = state.issueSource.url;
    link.target = "_blank";
    link.rel = "noopener";
    link.textContent = "Open";
    source.append(link);
  }
  wrapper.append(source);

  if (state.issues.length === 0) {
    wrapper.append(el("div", "changes-empty", "No open issues"));
    return wrapper;
  }

  const list = el("div", "issues-list");
  for (const issue of state.issues) {
    const item = el("article", "issue-item");
    item.setAttribute("role", "button");
    item.tabIndex = 0;
    item.addEventListener("click", () => openCreateWorktreeModal(issue));
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        openCreateWorktreeModal(issue);
      }
    });

    const topRow = el("div", "issue-item-top");
    topRow.append(
      el("span", "issue-display-id", issue.display_id),
      el("span", "issue-title", issue.title),
    );
    if (issue.url !== null) {
      const link = document.createElement("a");
      link.className = "issue-link";
      link.href = issue.url;
      link.target = "_blank";
      link.rel = "noopener";
      link.textContent = "Open";
      link.addEventListener("click", (event) => {
        event.stopPropagation();
      });
      topRow.append(link);
    }

    const bottomRow = el("div", "issue-item-bottom");
    bottomRow.append(
      el("span", "issue-state", issue.state),
      el("span", "issue-age", issue.updated_at === null ? "recently" : formatIssueAge(issue.updated_at)),
      el("span", "issue-cta", "Create worktree"),
    );

    item.append(topRow, bottomRow);
    list.append(item);
  }

  wrapper.append(list);
  return wrapper;
}

function buildTabButton(label: string, tab: "changes" | "issues", count: number): HTMLElement {
  const button = el("button", "changes-tab");
  button.type = "button";
  if (state.rightPanelTab === tab) {
    button.classList.add("active");
  }
  button.append(
    el("span", "changes-tab-label", label),
    el("span", "changes-tab-count", String(count)),
  );
  button.addEventListener("click", () => {
    setRightPanelTab(tab);
  });
  return button;
}

function formatIssueAge(updatedAt: string): string {
  const timestamp = Date.parse(updatedAt);
  if (Number.isNaN(timestamp)) {
    return updatedAt;
  }
  return formatAge(timestamp);
}
