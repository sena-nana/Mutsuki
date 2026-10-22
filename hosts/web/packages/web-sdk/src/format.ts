import { DEFAULT_LOCALE, type Locale } from "./i18n.js";

const DASH = "—";

function toDate(value: Date | number | string | null | undefined): Date | null {
  if (value == null || value === "") return null;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Absolute local date and time, e.g. `2026/10/22 13:05:09`. */
export function formatDateTime(
  value: Date | number | string | null | undefined,
  locale: Locale = DEFAULT_LOCALE,
): string {
  const date = toDate(value);
  if (!date) return DASH;
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date);
}

/** Relative to `now`, e.g. `3 分钟前` / `2 小时后`. */
export function formatRelativeTime(
  value: Date | number | string | null | undefined,
  now: Date | number = Date.now(),
  locale: Locale = DEFAULT_LOCALE,
): string {
  const date = toDate(value);
  if (!date) return DASH;
  const seconds = Math.round((date.getTime() - Number(now)) / 1000);
  const format = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ["year", 365 * 24 * 3600],
    ["month", 30 * 24 * 3600],
    ["day", 24 * 3600],
    ["hour", 3600],
    ["minute", 60],
  ];
  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) return format.format(Math.trunc(seconds / size), unit);
  }
  return format.format(seconds, "second");
}

/** Elapsed time in the two largest units, e.g. `1 小时 5 分` / `42 秒`. */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(Number(ms))) return DASH;
  const total = Math.max(0, Math.floor(Number(ms) / 1000));
  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (days) return `${days} 天 ${hours} 小时`;
  if (hours) return `${hours} 小时 ${minutes} 分`;
  if (minutes) return `${minutes} 分 ${seconds} 秒`;
  return `${seconds} 秒`;
}

/** Binary units, e.g. `1.5 MiB`. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(Number(bytes))) return DASH;
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = Math.max(0, Number(bytes));
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${Number(value.toFixed(digits))} ${units[unit]}`;
}
