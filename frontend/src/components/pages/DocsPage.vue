<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";

// Backend serves the docs; keeping the render path in the browser means
// operators editing docs/*.md see their edits after a rebuild of the
// deckwatch binary (the markdown is embedded via include_str!).
type DocEntry = { slug: string; title: string };

const route = useRoute();
const router = useRouter();

const pages = ref<DocEntry[]>([]);
const loadingIndex = ref(false);
const indexError = ref<string | null>(null);

const currentSlug = computed(() => (route.params.slug as string) || "");
const markdown = ref<string>("");
const loadingPage = ref(false);
const pageError = ref<string | null>(null);

async function fetchIndex() {
  loadingIndex.value = true;
  indexError.value = null;
  try {
    const res = await fetch("/api/docs/pages");
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const body = (await res.json()) as { pages: DocEntry[] };
    pages.value = body.pages;
    // If no slug in URL and we have pages, redirect to the first one so the
    // page never renders as an empty right-hand pane.
    if (!currentSlug.value && pages.value.length) {
      router.replace({ name: "Docs", params: { slug: pages.value[0].slug } });
    }
  } catch (e) {
    indexError.value = e instanceof Error ? e.message : String(e);
  } finally {
    loadingIndex.value = false;
  }
}

async function fetchPage(slug: string) {
  if (!slug) {
    markdown.value = "";
    return;
  }
  loadingPage.value = true;
  pageError.value = null;
  try {
    const res = await fetch(`/api/docs/pages/${encodeURIComponent(slug)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    markdown.value = await res.text();
  } catch (e) {
    pageError.value = e instanceof Error ? e.message : String(e);
    markdown.value = "";
  } finally {
    loadingPage.value = false;
  }
}

// Minimal markdown → HTML with escaping. Intentionally not pulling in a full
// markdown renderer to keep the frontend bundle lean — the docs are trusted
// (bundled with the binary via include_str!) so we don't need sanitisation,
// but we still HTML-escape user content out of habit and to make the output
// safe if someone ever swaps the source for user-editable text.
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderMarkdown(src: string): string {
  const lines = src.split(/\r?\n/);
  const out: string[] = [];
  let inCodeBlock = false;
  let codeLang = "";
  let codeBuf: string[] = [];
  let inList = false;
  let paraBuf: string[] = [];

  const flushPara = () => {
    if (paraBuf.length) {
      out.push(`<p>${renderInline(paraBuf.join(" "))}</p>`);
      paraBuf = [];
    }
  };
  const closeList = () => {
    if (inList) {
      out.push("</ul>");
      inList = false;
    }
  };

  for (const raw of lines) {
    const line = raw;
    if (inCodeBlock) {
      if (line.startsWith("```")) {
        out.push(
          `<pre class="dw-code"><code${codeLang ? ` class="lang-${escapeHtml(codeLang)}"` : ""}>${escapeHtml(codeBuf.join("\n"))}</code></pre>`,
        );
        codeBuf = [];
        codeLang = "";
        inCodeBlock = false;
      } else {
        codeBuf.push(line);
      }
      continue;
    }
    if (line.startsWith("```")) {
      flushPara();
      closeList();
      inCodeBlock = true;
      codeLang = line.slice(3).trim();
      continue;
    }
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) {
      flushPara();
      closeList();
      const level = h[1].length;
      out.push(`<h${level}>${renderInline(h[2])}</h${level}>`);
      continue;
    }
    const li = /^\s*[-*]\s+(.*)$/.exec(line);
    if (li) {
      flushPara();
      if (!inList) {
        out.push("<ul>");
        inList = true;
      }
      out.push(`<li>${renderInline(li[1])}</li>`);
      continue;
    }
    if (line.trim() === "") {
      flushPara();
      closeList();
      continue;
    }
    paraBuf.push(line);
  }
  flushPara();
  closeList();
  return out.join("\n");
}

function renderInline(text: string): string {
  // Escape first, then re-inject markup tokens so escaping never eats the
  // formatting brackets we're about to add.
  let s = escapeHtml(text);
  // Inline code
  s = s.replace(/`([^`]+)`/g, (_, c) => `<code>${c}</code>`);
  // Bold
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  // Italic (single underscore or single asterisk, non-greedy)
  s = s.replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>");
  // Links [text](url) — url is already escaped, which is fine
  s = s.replace(
    /\[([^\]]+)\]\(([^)]+)\)/g,
    '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>',
  );
  return s;
}

const rendered = computed(() => renderMarkdown(markdown.value));

onMounted(fetchIndex);
watch(currentSlug, (slug) => fetchPage(slug), { immediate: true });
</script>

<template>
  <v-row>
    <v-col cols="12" md="3">
      <v-card variant="outlined">
        <v-list density="compact" nav>
          <v-list-subheader>Documentation</v-list-subheader>
          <v-list-item v-if="loadingIndex" prepend-icon="mdi-loading">
            Loading…
          </v-list-item>
          <v-list-item v-if="indexError" prepend-icon="mdi-alert-circle" color="error">
            {{ indexError }}
          </v-list-item>
          <v-list-item
            v-for="p in pages"
            :key="p.slug"
            :to="{ name: 'Docs', params: { slug: p.slug } }"
            :active="p.slug === currentSlug"
          >
            <v-list-item-title>{{ p.title }}</v-list-item-title>
          </v-list-item>
          <v-divider class="my-2" />
          <v-list-item
            href="/api/docs"
            target="_blank"
            rel="noopener noreferrer"
            prepend-icon="mdi-api"
          >
            <v-list-item-title>API Reference (Swagger)</v-list-item-title>
          </v-list-item>
        </v-list>
      </v-card>
    </v-col>

    <v-col cols="12" md="9">
      <v-card variant="outlined">
        <v-card-text>
          <v-progress-linear v-if="loadingPage" indeterminate />
          <v-alert v-if="pageError" type="error" class="mb-4">
            {{ pageError }}
          </v-alert>
          <div v-if="!currentSlug && !loadingIndex" class="text-medium-emphasis">
            Pick a document from the sidebar.
          </div>
          <article class="dw-markdown" v-html="rendered" />
        </v-card-text>
      </v-card>
    </v-col>
  </v-row>
</template>

<style scoped>
.dw-markdown :deep(h1) {
  font-size: 1.75rem;
  margin: 0 0 1rem;
}
.dw-markdown :deep(h2) {
  font-size: 1.4rem;
  margin: 1.5rem 0 0.75rem;
}
.dw-markdown :deep(h3) {
  font-size: 1.15rem;
  margin: 1.25rem 0 0.5rem;
}
.dw-markdown :deep(p) {
  line-height: 1.6;
  margin: 0 0 1rem;
}
.dw-markdown :deep(ul) {
  padding-left: 1.5rem;
  margin: 0 0 1rem;
}
.dw-markdown :deep(code) {
  background: rgba(127, 127, 127, 0.12);
  padding: 0.1rem 0.35rem;
  border-radius: 3px;
  font-family: "SFMono-Regular", Consolas, Menlo, monospace;
  font-size: 0.9em;
}
.dw-markdown :deep(pre.dw-code) {
  background: rgba(127, 127, 127, 0.12);
  padding: 0.75rem 1rem;
  border-radius: 4px;
  overflow-x: auto;
  margin: 0 0 1rem;
}
.dw-markdown :deep(pre.dw-code code) {
  background: transparent;
  padding: 0;
}
.dw-markdown :deep(a) {
  color: rgb(var(--v-theme-primary));
}
</style>
