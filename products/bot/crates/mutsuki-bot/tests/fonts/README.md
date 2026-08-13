# Test fonts

`NotoSansSC-Test.ttf` is a test-only subset of Google Fonts' `Noto Sans SC`
variable font. It contains only the Latin and CJK glyphs needed by image-render
regression cards. Production deployments must supply their own absolute font
file paths via `ImageRenderConfig.font_files`; the renderer never reads system
fonts.
