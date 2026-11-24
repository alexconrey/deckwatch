// Keep this first. It declares the order of Vuetify 4's cascade layers before
// any component CSS reaches the bundle. Without it, App.vue's components (VBtn,
// VApp, ...) are bundled ahead of vuetify/styles, so the reset layer ends up
// outranking component styles (e.g. `button { font: inherit }` beats
// .v-btn's font size). See https://vuetifyjs.com/en/styles/layers/
import "vuetify/styles/layers";
import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { vuetify } from "./plugins/vuetify";
import { router } from "./router";

import "@mdi/font/css/materialdesignicons.css";
import "./styles/index.css";

const app = createApp(App);
app.use(createPinia());
app.use(vuetify);
app.use(router);
app.mount("#app");
