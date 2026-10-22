import type { App } from "vue";
import { installOverlays, type OverlayStore } from "./overlay/store";

export { applyTheme, resolveTheme, type MutsukiTheme } from "./theme";
export { uiMessages, useUiText } from "./messages";
export { ICON_PATHS, type IconName } from "./components/icons";
export type {
  MkAsyncSource,
  MkColumn,
  MkErrorLike,
  MkKeyValueItem,
  MkMenuItem,
  MkOption,
  MkTab,
  MkTimelineItem,
  MkTone,
} from "./types";

// Base
export { default as MkButton } from "./components/MkButton.vue";
export { default as MkIconButton } from "./components/MkIconButton.vue";
export { default as MkIcon } from "./components/MkIcon.vue";
export { default as MkCard } from "./components/MkCard.vue";
export { default as MkPageHeader } from "./components/MkPageHeader.vue";
export { default as MkSection } from "./components/MkSection.vue";
export { default as MkTabs } from "./components/MkTabs.vue";
export { default as MkSplitPane } from "./components/MkSplitPane.vue";
export { default as MkDrawer } from "./components/MkDrawer.vue";
// Data
export { default as MkDataTable } from "./components/MkDataTable.vue";
export { default as MkKeyValue } from "./components/MkKeyValue.vue";
export { default as MkStatusBadge } from "./components/MkStatusBadge.vue";
export { default as MkRelativeTime } from "./components/MkRelativeTime.vue";
export { default as MkJsonView } from "./components/MkJsonView.vue";
export { default as MkCodeBlock } from "./components/MkCodeBlock.vue";
export { default as MkCopyButton } from "./components/MkCopyButton.vue";
export { default as MkTimeline } from "./components/MkTimeline.vue";
// State
export { default as MkEmptyState } from "./components/MkEmptyState.vue";
export { default as MkErrorState } from "./components/MkErrorState.vue";
export { default as MkSkeleton } from "./components/MkSkeleton.vue";
export { default as MkAsync } from "./components/MkAsync.vue";
// Forms
export { default as MkField } from "./components/MkField.vue";
export { default as MkTextInput } from "./components/MkTextInput.vue";
export { default as MkTextarea } from "./components/MkTextarea.vue";
export { default as MkNumberInput } from "./components/MkNumberInput.vue";
export { default as MkSelect } from "./components/MkSelect.vue";
export { default as MkSwitch } from "./components/MkSwitch.vue";
export { default as MkCheckbox } from "./components/MkCheckbox.vue";
export { default as MkRadioGroup } from "./components/MkRadioGroup.vue";
export { default as MkSegmented } from "./components/MkSegmented.vue";
export { default as MkSecretInput } from "./components/MkSecretInput.vue";
export { default as MkSearchInput } from "./components/MkSearchInput.vue";
// Overlays
export { default as MkDialog } from "./components/MkDialog.vue";
export { default as MkMenu } from "./components/MkMenu.vue";
export { default as MkTooltip } from "./components/MkTooltip.vue";
export { default as MkOverlayHost } from "./overlay/MkOverlayHost.vue";
// Shell accessibility
export { default as MkSkipLink } from "./components/MkSkipLink.vue";
export { default as MkLiveRegion } from "./components/MkLiveRegion.vue";

export {
  MAX_TOASTS,
  createOverlayStore,
  installOverlays,
  useConfirm,
  usePrompt,
  useToast,
  type ConfirmOptions,
  type OverlayStore,
  type PromptOptions,
  type Toaster,
  type ToastOptions,
} from "./overlay/store";

/** Installs app-wide UI services (the overlay queue behind confirm/prompt/toast). */
export function installMutsukiUi(app: App): { overlays: OverlayStore } {
  return { overlays: installOverlays(app) };
}
