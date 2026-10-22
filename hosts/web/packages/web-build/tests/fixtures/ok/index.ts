import { defineComponent, h } from "vue";
import Panel from "./Panel.vue";

export default {
  id: "fixture",
  setup() {
    return defineComponent({ render: () => h(Panel, { label: "fixture" }) });
  },
};
