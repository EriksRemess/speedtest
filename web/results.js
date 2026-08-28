const RESULTS_KEY = "results";
const PAGE_SIZE = 10;
const DATE_FORMAT = new Intl.DateTimeFormat(i18n.locale(), {
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
    i18n._("theme.switch", { theme: theme === "mocha" ? "Latte" : "Mocha" }),
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

function rows(result, index) {
  const tr = document.createElement("tr");
  tr.className = "result-row";
  const date = document.createElement("td");
  const dateText = DATE_FORMAT.format(new Date(result.timestamp));
  if (hasSpeedSamples(result)) {
    const chartId = `result-chart-${index}`;
    const toggle = document.createElement("button");
    toggle.className = "result-toggle";
    toggle.type = "button";
    toggle.textContent = dateText;
    toggle.setAttribute("aria-expanded", "false");
    toggle.setAttribute("aria-controls", chartId);
    date.append(toggle);

    const detail = document.createElement("tr");
    detail.className = "result-chart-row";
    detail.hidden = true;
    const chartCell = document.createElement("td");
    chartCell.colSpan = 5;
    const chart = document.createElement("div");
    chart.id = chartId;
    chart.className = "saved-result-chart";
    chartCell.append(chart);
    detail.append(chartCell);
    const toggleDetail = () => {
      detail.hidden = !detail.hidden;
      tr.classList.toggle("expanded", !detail.hidden);
      toggle.setAttribute("aria-expanded", String(!detail.hidden));
      if (!detail.hidden && chart.childElementCount === 0) {
        renderSpeedChart(chart, result);
      }
    };
    tr.addEventListener("click", toggleDetail);
    tr.append(date);
    appendMetrics(tr, result);
    return [tr, detail];
  }

  const plainDate = document.createElement("span");
  plainDate.className = "result-date";
  plainDate.textContent = dateText;
  date.append(plainDate);
  tr.append(date);
  appendMetrics(tr, result);
  return [tr];
}

function appendMetrics(tr, result) {
  for (
    const key of [
      "latency",
      "jitter",
      "download",
      "upload",
    ]
  ) {
    const cell = document.createElement("td");
    cell.dataset.label = i18n._(`metric.${key}`);
    if (key === "download" || key === "upload") {
      const rate = i18n.rate(result[key]);
      const unit = document.createElement("small");
      unit.className = "result-unit";
      unit.textContent = rate.unit;
      cell.append(rate.number, " ", unit);
    } else {
      const unit = document.createElement("small");
      unit.className = "result-time-unit";
      unit.textContent = "ms";
      cell.append(metric(result[key]), " ", unit);
    }
    tr.append(cell);
  }
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
    ...results.slice(start, start + PAGE_SIZE).flatMap((result, index) =>
      rows(result, start + index)
    ),
  );

  const empty = results.length === 0;
  ui.empty.hidden = !empty;
  ui.tableWrap.hidden = empty;
  ui.pagination.hidden = empty || pageCount === 1;
  ui.clear.disabled = empty;
  ui.previous.disabled = currentPage === 1;
  ui.next.disabled = currentPage === pageCount;
  ui.page.textContent = i18n._("results.page", {
    current: currentPage,
    total: pageCount,
  });
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
  if (!confirm(i18n._("results.clearConfirm"))) return;
  try {
    localStorage.removeItem(RESULTS_KEY);
  } catch {
    // Rendering an empty list is still useful when storage is unavailable.
  }
  currentPage = 1;
  render();
});

render();
