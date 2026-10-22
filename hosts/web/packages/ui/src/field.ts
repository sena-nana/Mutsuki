import { computed, inject, useId, type ComputedRef, type InjectionKey } from "vue";

/** What `MkField` tells the control inside it: its id, description ids and validity. */
export interface FieldContext {
  id: string;
  describedBy: ComputedRef<string | undefined>;
  invalid: ComputedRef<boolean>;
  required: ComputedRef<boolean>;
}

export const FIELD_CONTEXT: InjectionKey<FieldContext> = Symbol("mk.field");

/** A control's id/ARIA wiring, from the enclosing field or standalone. */
export function useFieldControl(own: { id?: string; invalid?: () => boolean } = {}) {
  const field = inject(FIELD_CONTEXT, null);
  const fallbackId = useId();
  return {
    id: computed(() => own.id ?? field?.id ?? fallbackId),
    describedBy: computed(() => field?.describedBy.value),
    invalid: computed(() => own.invalid?.() || field?.invalid.value || false),
    required: computed(() => field?.required.value ?? false),
  };
}
