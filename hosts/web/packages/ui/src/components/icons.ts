/** 24×24 stroke paths for `MkIcon`. Product shell and components share this one set. */
export const ICON_PATHS = {
  home: "M3 11.5 12 4l9 7.5V21h-6v-6H9v6H3z",
  bot: "M7 8h10a4 4 0 0 1 4 4v5a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4v-5a4 4 0 0 1 4-4Zm2 5v2m6-2v2M12 8V4m-2 0h4",
  flow: "M5 4h5v5H5zM14 15h5v5h-5zM7.5 9v3a5 5 0 0 0 5 5H14M14 4h5v5h-5z",
  system:
    "M12 3v3m0 12v3M3 12h3m12 0h3M5.6 5.6l2.1 2.1m8.6 8.6 2.1 2.1m0-12.8-2.1 2.1m-8.6 8.6-2.1 2.1M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
  config: "M5 4h14v5H5zM5 13h14v7H5zM8 6.5h4M8 16h8",
  settings: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm8 4 2-1-2-3-2 .4-1.4-1.4.4-2-3-2-1 2h-2l-1-2-3 2 .4 2L6 8.4 4 8l-2 3 2 1v2l-2 1 2 3 2-.4L7.4 19 7 21l3 2 1-2h2l1 2 3-2-.4-2 1.4-1.4 2 .4 2-3-2-1Z",
  sandbox: "M4 8h16v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8Zm0 0 8-4 8 4M8 12h8M8 16h5",
  data: "M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3Zm0 0v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  refresh: "M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  check: "m5 12 5 5L20 7",
  close: "M6 6l12 12M18 6 6 18",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14Zm9 16-4-4",
  eye: "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12Zm10-3a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
  "eye-off": "M3 3l18 18M10.6 5.1A10 10 0 0 1 12 5c6.4 0 10 7 10 7a17 17 0 0 1-3.2 4.1M6.6 6.6A17 17 0 0 0 2 12s3.6 7 10 7a9.7 9.7 0 0 0 5.4-1.6M9.9 9.9a3 3 0 0 0 4.2 4.2",
  "chevron-down": "m6 9 6 6 6-6",
  "chevron-right": "m9 6 6 6-6 6",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  plus: "M12 5v14M5 12h14",
  trash: "M4 7h16M9 7V4h6v3m-8 0 1 13h8l1-13",
  alert: "M12 3 2 20h20L12 3Zm0 6v5m0 3h.01",
  info: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Zm0-11v6m0-9h.01",
  external: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
  menu: "M4 6h16M4 12h16M4 18h16",
  chat: "M4 5h16v11H9l-5 4V5Zm4 5h8M8 13h5",
} as const;

export type IconName = keyof typeof ICON_PATHS;
