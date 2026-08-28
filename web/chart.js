function hasSpeedSamples(result) {
  return [result?.downloadSamples, result?.uploadSamples].some((samples) =>
    Array.isArray(samples) && samples.some((sample) =>
      Array.isArray(sample) && sample.length === 2 &&
      Number.isFinite(sample[0]) && sample[0] >= 0 &&
      Number.isFinite(sample[1]) && sample[1] >= 0
    )
  );
}

function renderSpeedChart(container, result) {
  const series = [
    {
      name: i18n._("metric.download"),
      descriptionKey: "chart.downloadDescription",
      className: "download",
      average: result.download,
      samples: validSpeedSamples(result.downloadSamples),
    },
    {
      name: i18n._("metric.upload"),
      descriptionKey: "chart.uploadDescription",
      className: "upload",
      average: result.upload,
      samples: validSpeedSamples(result.uploadSamples),
    },
  ].filter((item) => item.samples.length > 0 && Number.isFinite(item.average));
  if (series.length === 0) return false;

  const wrapper = document.createElement("div");
  wrapper.className = "speed-chart";
  const heading = document.createElement("div");
  heading.className = "chart-heading";
  const title = document.createElement("strong");
  title.textContent = i18n._("chart.title");
  const interval = document.createElement("span");
  interval.textContent = i18n._("chart.interval");
  heading.append(title, interval);
  wrapper.append(heading);

  for (const item of series) {
    const panel = document.createElement("section");
    panel.className = "chart-panel";
    const panelHeading = document.createElement("div");
    panelHeading.className = "chart-series-heading";
    const label = document.createElement("span");
    const marker = document.createElement("i");
    marker.className = item.className;
    label.append(marker, item.name);
    const average = document.createElement("strong");
    average.textContent = i18n._("chart.average", {
      speed: i18n.rate(item.average).text,
    });
    panelHeading.append(label, average);
    panel.append(panelHeading, speedWave(item));
    wrapper.append(panel);
  }
  container.replaceChildren(wrapper);
  return true;
}

function speedWave(item) {
  const width = 600;
  const height = 100;
  const plot = { left: 0, right: 0, top: 6, bottom: 18 };
  const plotHeight = height - plot.top - plot.bottom;
  const maxSeconds = Math.max(
    5,
    ...item.samples.map((sample) => sample[0]),
  );
  const speeds = [item.average, ...item.samples.map((sample) => sample[1])];
  const observedMin = Math.min(...speeds);
  const observedMax = Math.max(...speeds);
  const center = (observedMin + observedMax) / 2;
  const range = Math.max(
    (observedMax - observedMin) * 1.2,
    item.average * 0.1,
    1,
  );
  let minSpeed = center - range / 2;
  let maxSpeed = center + range / 2;
  if (minSpeed < 0) {
    maxSpeed -= minSpeed;
    minSpeed = 0;
  }
  const x = (seconds) => plot.left + seconds / maxSeconds * width;
  const y = (mbps) => plot.top +
    (maxSpeed - mbps) / (maxSpeed - minSpeed) * plotHeight;
  const svg = svgElement("svg", {
    viewBox: `0 0 ${width} ${height}`,
    role: "img",
    "aria-label": i18n._(item.descriptionKey, {
      speed: i18n.rate(item.average).text,
    }),
  });
  const color = item.className === "download" ? "var(--blue)" : "var(--green)";
  svg.append(svgElement("line", {
    class: `chart-average ${item.className}`,
    stroke: color,
    x1: 0,
    x2: width,
    y1: y(item.average),
    y2: y(item.average),
  }));
  const samples = [
    [0, item.samples[0][1]],
    ...item.samples,
    [maxSeconds, item.samples.at(-1)[1]],
  ];
  const points = samples.map((sample) => ({
    x: x(sample[0]),
    y: y(Math.min(sample[1], maxSpeed)),
  }));
  svg.append(svgElement("path", {
    class: `chart-line ${item.className}`,
    d: smoothPath(points),
    fill: "none",
    stroke: color,
    "stroke-width": 2,
  }));
  const tooltip = speedTooltip(svg);
  for (const sample of item.samples) {
    const pointX = x(sample[0]);
    const pointY = y(sample[1]);
    const point = svgElement("g", { class: "chart-point" });
    const dot = svgElement("circle", {
      class: `chart-dot ${item.className}`,
      cx: pointX,
      cy: pointY,
      r: 3,
      fill: color,
    });
    const hit = svgElement("circle", {
      class: "chart-hit-target",
      cx: pointX,
      cy: pointY,
      r: 11,
      tabindex: 0,
      role: "button",
      "aria-label": i18n._("chart.sample", {
        seconds: formatSeconds(sample[0]),
        speed: i18n.rate(sample[1]).text,
      }),
    });
    const show = () => tooltip.show(pointX, pointY, sample);
    hit.addEventListener("pointerenter", show);
    hit.addEventListener("pointerleave", tooltip.hide);
    hit.addEventListener("focus", show);
    hit.addEventListener("blur", tooltip.hide);
    point.append(dot, hit);
    svg.append(point);
  }
  svg.append(tooltip.element);
  svg.append(
    chartText("0s", 0, height - 3, "start"),
    chartText(`${formatSeconds(maxSeconds)}s`, width, height - 3, "end"),
  );
  return svg;
}

function smoothPath(points) {
  if (points.length === 0) return "";
  let path = `M${points[0].x.toFixed(2)},${points[0].y.toFixed(2)}`;
  for (let index = 0; index < points.length - 1; index++) {
    const previous = points[Math.max(0, index - 1)];
    const start = points[index];
    const end = points[index + 1];
    const next = points[Math.min(points.length - 1, index + 2)];
    const control1 = {
      x: start.x + (end.x - previous.x) / 6,
      y: start.y + (end.y - previous.y) / 6,
    };
    const control2 = {
      x: end.x - (next.x - start.x) / 6,
      y: end.y - (next.y - start.y) / 6,
    };
    path += ` C${control1.x.toFixed(2)},${control1.y.toFixed(2)} ${
      control2.x.toFixed(2)
    },${control2.y.toFixed(2)} ${end.x.toFixed(2)},${end.y.toFixed(2)}`;
  }
  return path;
}

function speedTooltip(svg) {
  const element = svgElement("g", {
    class: "chart-tooltip",
    visibility: "hidden",
    "aria-hidden": "true",
  });
  const box = svgElement("rect", {
    width: 128,
    height: 25,
    rx: 5,
  });
  const label = chartText("", 0, 0, "middle");
  label.classList.add("chart-tooltip-label");
  element.append(box, label);
  return {
    element,
    show(pointX, pointY, sample) {
      const boxX = Math.max(0, Math.min(600 - 128, pointX - 64));
      const boxY = pointY < 38 ? pointY + 10 : pointY - 34;
      box.setAttribute("x", boxX);
      box.setAttribute("y", boxY);
      label.setAttribute("x", boxX + 64);
      label.setAttribute("y", boxY + 16);
      label.textContent = `${i18n.rate(sample[1]).text} · ${
        formatSeconds(sample[0])
      }s`;
      element.setAttribute("visibility", "visible");
      svg.append(element);
    },
    hide() {
      element.setAttribute("visibility", "hidden");
    },
  };
}

function validSpeedSamples(samples) {
  if (!Array.isArray(samples)) return [];
  return samples.filter((sample) =>
    Array.isArray(sample) && sample.length === 2 &&
    Number.isFinite(sample[0]) && sample[0] >= 0 &&
    Number.isFinite(sample[1]) && sample[1] >= 0
  ).slice(0, 100);
}

function formatSeconds(value) {
  return new Intl.NumberFormat(i18n.locale(), {
    maximumFractionDigits: 1,
  }).format(value);
}

function chartText(value, x, y, anchor) {
  const text = svgElement("text", {
    class: "chart-label",
    fill: "var(--subtext)",
    x,
    y,
    "text-anchor": anchor,
  });
  text.textContent = value;
  return text;
}

function svgElement(name, attrs) {
  const element = document.createElementNS("http://www.w3.org/2000/svg", name);
  for (const [key, value] of Object.entries(attrs)) {
    element.setAttribute(key, value);
  }
  return element;
}
