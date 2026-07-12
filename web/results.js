const RESULTS_KEY = "speedtest-results";
const PAGE_SIZE = 10;
const DATE_FORMAT = new Intl.DateTimeFormat("en-GB", {
  day: "2-digit",
  month: "short",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

const $ = (id) => document.getElementById(id);
const ui = {
  theme: $("theme"),
  clear: $("clear"),
  empty: $("empty"),
  tableWrap: $("table-wrap"),
  results: $("results"),
  pagination: $("pagination"),
  previous: $("previous"),
  next: $("next"),
  page: $("page"),
  sortButtons: [...document.querySelectorAll(".sort-button")],
};

$("host").textContent = location.host;
let currentPage = Math.max(
  1,
  Number.parseInt(
    new URLSearchParams(location.search).get("page") || "1",
    10,
  ) || 1,
);
let sortKey = "timestamp";
let sortDirection = "descending";

function readStorage(key, fallback) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  ui.theme.setAttribute(
    "aria-label",
    `Switch to ${theme === "mocha" ? "Latte" : "Mocha"} theme`,
  );
  try {
    localStorage.setItem("theme", theme);
  } catch {
    // Theme persistence is optional.
  }
}

const savedTheme = readStorage("theme", "");
setTheme(
  savedTheme === "mocha" || savedTheme === "latte"
    ? savedTheme
    : (matchMedia("(prefers-color-scheme: dark)").matches ? "mocha" : "latte"),
);
ui.theme.addEventListener("click", () => {
  setTheme(
    document.documentElement.dataset.theme === "mocha" ? "latte" : "mocha",
  );
});

function loadResults() {
  try {
    const results = JSON.parse(readStorage(RESULTS_KEY, "[]"));
    return Array.isArray(results) ? results.filter(validResult) : [];
  } catch {
    return [];
  }
}

function validResult(result) {
  return result && typeof result.timestamp === "string" &&
    ["latency", "jitter", "download", "upload"].every((key) =>
      Number.isFinite(result[key])
    );
}

function metric(value) {
  return value.toFixed(value < 100 ? 1 : 0);
}

function row(result) {
  const tr = document.createElement("tr");
  const date = document.createElement("td");
  const dateText = document.createElement("span");
  dateText.className = "result-date";
  dateText.textContent = DATE_FORMAT.format(new Date(result.timestamp));
  date.append(dateText);
  tr.append(date);

  for (
    const key of [
      "latency",
      "jitter",
      "download",
      "upload",
    ]
  ) {
    const cell = document.createElement("td");
    cell.textContent = metric(result[key]);
    tr.append(cell);
  }
  return tr;
}

function render() {
  const direction = sortDirection === "ascending" ? 1 : -1;
  const results = loadResults().sort((left, right) => {
    const leftValue = sortKey === "timestamp"
      ? Date.parse(left.timestamp)
      : left[sortKey];
    const rightValue = sortKey === "timestamp"
      ? Date.parse(right.timestamp)
      : right[sortKey];
    return (leftValue - rightValue) * direction;
  });
  const pageCount = Math.max(1, Math.ceil(results.length / PAGE_SIZE));
  currentPage = Math.min(currentPage, pageCount);
  const start = (currentPage - 1) * PAGE_SIZE;
  ui.results.replaceChildren(
    ...results.slice(start, start + PAGE_SIZE).map(row),
  );

  const empty = results.length === 0;
  ui.empty.hidden = !empty;
  ui.tableWrap.hidden = empty;
  ui.pagination.hidden = empty || pageCount === 1;
  ui.clear.disabled = empty;
  ui.previous.disabled = currentPage === 1;
  ui.next.disabled = currentPage === pageCount;
  ui.page.textContent = `Page ${currentPage} of ${pageCount}`;
  for (const button of ui.sortButtons) {
    const active = button.dataset.sort === sortKey;
    const header = button.closest("th");
    header.setAttribute("aria-sort", active ? sortDirection : "none");
    button.dataset.direction = active
      ? (sortDirection === "ascending" ? "up" : "down")
      : "";
  }

  const url = currentPage === 1 ? "/results" : `/results?page=${currentPage}`;
  history.replaceState(null, "", url);
}

for (const button of ui.sortButtons) {
  button.addEventListener("click", () => {
    const nextKey = button.dataset.sort;
    if (sortKey === nextKey) {
      sortDirection = sortDirection === "descending"
        ? "ascending"
        : "descending";
    } else {
      sortKey = nextKey;
      sortDirection = "descending";
    }
    currentPage = 1;
    render();
  });
}

ui.previous.addEventListener("click", () => {
  currentPage -= 1;
  render();
});
ui.next.addEventListener("click", () => {
  currentPage += 1;
  render();
});
ui.clear.addEventListener("click", () => {
  if (!confirm("Clear all saved results?")) return;
  try {
    localStorage.removeItem(RESULTS_KEY);
  } catch {
    // Rendering an empty list is still useful when storage is unavailable.
  }
  currentPage = 1;
  render();
});

render();
