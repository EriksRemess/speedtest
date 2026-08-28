const COOKIE = "language";
const LANGUAGES = ["en", "lv"];
const catalogs = window.translations;

const i18n = {
  language: detectLanguage(),
  _(key, values = {}) {
    const template = catalogs[this.language][key] ?? catalogs.en[key] ?? key;
    return Object.entries(values).reduce(
      (text, [name, value]) => text.replaceAll(`{${name}}`, value),
      template,
    );
  },
  locale() {
    return this.language === "lv" ? "lv-LV" : "en-GB";
  },
  rate(mbps) {
    const units = ["bps", "Kbps", "Mbps", "Gbps", "Tbps"];
    let value = mbps;
    let unit = 2;
    while (value >= 1000 && unit < units.length - 1) {
      value /= 1000;
      unit += 1;
    }
    while (value > 0 && value < 1 && unit > 0) {
      value *= 1000;
      unit -= 1;
    }
    const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
    const number = new Intl.NumberFormat(this.locale(), {
      maximumFractionDigits: digits,
    }).format(value);
    return { number, unit: units[unit], text: `${number} ${units[unit]}` };
  },
};

function detectLanguage() {
  let cookie;
  try {
    cookie = document.cookie.split(";").map((part) => part.trim())
      .find((part) => part.startsWith(`${COOKIE}=`));
  } catch {
    // Cookie restrictions should not prevent language detection.
  }
  if (cookie) {
    const value = decodeURIComponent(cookie.slice(COOKIE.length + 1));
    if (LANGUAGES.includes(value)) return value;
  }
  return navigator.language.toLowerCase().startsWith("lv") ? "lv" : "en";
}

function applyTranslations() {
  document.documentElement.lang = i18n.language;
  for (const element of document.querySelectorAll("[data-i18n]")) {
    element.textContent = i18n._(element.dataset.i18n);
  }
  for (
    const element of document.querySelectorAll("[data-i18n-aria-label]")
  ) {
    element.setAttribute("aria-label", i18n._(element.dataset.i18nAriaLabel));
  }

  const selector = document.getElementById("language");
  if (!selector) return;
  selector.value = i18n.language;
  selector.addEventListener("change", () => {
    const language = LANGUAGES.includes(selector.value)
      ? selector.value
      : "en";
    try {
      document.cookie = `${COOKIE}=${encodeURIComponent(language)}; ` +
        "Path=/; Max-Age=31536000; SameSite=Lax";
    } catch {
      // The selection still applies for this reload when cookies are available.
    }
    location.reload();
  });
}

applyTranslations();
