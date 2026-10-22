import { inject, ref, type App, type InjectionKey, type Ref } from "vue";

export type Locale = "zh-CN";
export const SUPPORTED_LOCALES: readonly Locale[] = ["zh-CN"];
export const DEFAULT_LOCALE: Locale = "zh-CN";

/** Nested message tree; leaves are templates with `{name}` placeholders. */
export interface MessageTree {
  [key: string]: string | MessageTree;
}

/** Messages for every supported locale. Each locale must define the same keys. */
export type LocaleMessages<T extends MessageTree = MessageTree> = Record<Locale, T>;

/** Dot paths of a message tree, e.g. `overview.title`; any string for an untyped tree. */
export type MessageKey<T> = string extends keyof T
  ? string
  : {
      [K in keyof T & string]: T[K] extends string ? K : `${K}.${MessageKey<T[K]>}`;
    }[keyof T & string];

/** Declares an owner's messages; the type keeps `t()` keys checked at compile time. */
export function defineMessages<T extends MessageTree>(messages: LocaleMessages<T>): LocaleMessages<T> {
  const problems = checkMessageKeys(messages);
  if (problems.length) throw new Error(`message keys differ between locales: ${problems.join(", ")}`);
  return messages;
}

function flatten(tree: MessageTree, prefix = "", out = new Map<string, string>()): Map<string, string> {
  for (const [key, value] of Object.entries(tree)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") out.set(path, value);
    else flatten(value, path, out);
  }
  return out;
}

/** Keys present in one locale but missing in another, as `locale:key`. Empty when consistent. */
export function checkMessageKeys(messages: Partial<Record<string, MessageTree>>): string[] {
  const flat = Object.entries(messages).map(
    ([locale, tree]) => [locale, new Set(flatten(tree ?? {}).keys())] as const,
  );
  const all = new Set(flat.flatMap(([, keys]) => [...keys]));
  const missing: string[] = [];
  for (const [locale, keys] of flat) {
    for (const key of all) if (!keys.has(key)) missing.push(`${locale}:${key}`);
  }
  return missing.sort();
}

function interpolate(template: string, params?: Record<string, string | number>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]) : match,
  );
}

export interface I18n<T extends MessageTree = MessageTree> {
  readonly locale: Ref<Locale>;
  t(key: MessageKey<T>, params?: Record<string, string | number>): string;
  /** Adds an owner's namespace (`extension id` or short name) to the catalog. */
  extend<N extends string, U extends MessageTree>(
    namespace: N,
    messages: LocaleMessages<U>,
  ): I18n<T & { [K in N]: U }>;
}

export function createI18n<T extends MessageTree>(
  options: { locale?: Locale; messages?: LocaleMessages<T> } = {},
): I18n<T> {
  const locale = ref<Locale>(options.locale ?? DEFAULT_LOCALE);
  const catalogs = new Map<Locale, Map<string, string>>(
    SUPPORTED_LOCALES.map((code) => [code, flatten((options.messages?.[code] ?? {}) as MessageTree)]),
  );
  const warned = new Set<string>();
  const i18n: I18n<T> = {
    locale,
    t(key, params) {
      const template = catalogs.get(locale.value)?.get(key);
      if (template === undefined) {
        if (!warned.has(key)) {
          warned.add(key);
          console.warn(`[mutsuki i18n] missing message ${locale.value}:${key}`);
        }
        return key;
      }
      return interpolate(template, params);
    },
    extend(namespace, messages) {
      const problems = checkMessageKeys(messages);
      if (problems.length) {
        throw new Error(`message keys differ between locales: ${problems.join(", ")}`);
      }
      for (const code of SUPPORTED_LOCALES) {
        const catalog = catalogs.get(code)!;
        for (const [key, value] of flatten(messages[code], namespace)) {
          if (catalog.has(key)) throw new Error(`duplicate message key ${code}:${key}`);
          catalog.set(key, value);
        }
      }
      return i18n as unknown as I18n<T & { [K in typeof namespace]: (typeof messages)["zh-CN"] }>;
    },
  };
  return i18n;
}

export const MUTSUKI_I18N: InjectionKey<I18n> = Symbol("mutsuki.i18n");

export function provideI18n(app: App, i18n: I18n): void {
  app.provide(MUTSUKI_I18N, i18n);
}

let fallback: I18n | undefined;

/** The shell's i18n; outside a shell an empty zh-CN catalog that echoes keys. */
export function useI18n<T extends MessageTree = MessageTree>(): I18n<T> {
  const provided = inject(MUTSUKI_I18N, null);
  if (provided) return provided as unknown as I18n<T>;
  fallback ??= createI18n();
  return fallback as unknown as I18n<T>;
}
