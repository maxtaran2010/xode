import { For } from "solid-js";
import { PRESETS, resolvedTokens, TOKENS } from "../../lib/theme";
import { cfg, Field, Group, NumberField, Page, patch, TextField, Toggle } from "./controls";

const isHex = (v: string) => /^#[0-9a-f]{6}$/i.test(v);

export default function ThemePage() {
  const th = () => cfg().theme;
  const tokens = () => resolvedTokens(th());
  return (
    <Page title="Theme">
      <Group title="Preset">
        <div class="presets">
          <For each={Object.entries(PRESETS)}>
            {([id, p]) => (
              <button class="preset" classList={{ active: th().preset === id }} onClick={() => patch((c) => (c.theme.preset = id))}>
                <span class="preset-swatch" style={{ background: p.tokens.bg }}>
                  <i style={{ background: p.tokens["panel-solid"] }} />
                  <i style={{ background: p.tokens.elev }} />
                  <i style={{ background: p.tokens.accent }} />
                  <i style={{ background: p.tokens.text }} />
                </span>
                <span>{p.label}</span>
              </button>
            )}
          </For>
        </div>
      </Group>
      <Group>
        <Field label="Blur">
          <Toggle value={th().blur} onChange={(v) => patch((c) => (c.theme.blur = v))} />
        </Field>
        <Field label="Font size">
          <NumberField value={th().font_size} onChange={(v) => patch((c) => (c.theme.font_size = Math.max(11, Math.min(20, v ?? 14))))} min={11} max={20} />
        </Field>
        <Field label="Mono font">
          <TextField value={th().mono_font} onChange={(v) => patch((c) => (c.theme.mono_font = v))} placeholder="Cascadia Code" width={220} />
        </Field>
      </Group>
      <Group
        title="Colors"
        actions={
          <button class="btn ghost sm" onClick={() => patch((c) => (c.theme.tokens = {}))}>
            Reset
          </button>
        }
      >
        <div class="token-grid">
          <For each={[...TOKENS]}>
            {(k) => (
              <div class="token-row" classList={{ changed: k in th().tokens }}>
                <label class="token-swatch" style={{ background: tokens()[k] }}>
                  <input
                    type="color"
                    value={isHex(tokens()[k]) ? tokens()[k] : "#000000"}
                    onInput={(e) => {
                      const v = e.currentTarget.value;
                      patch((c) => (c.theme.tokens[k] = v));
                    }}
                  />
                </label>
                <span class="token-name mono">--{k}</span>
                <input
                  class="input mono token-value"
                  value={tokens()[k]}
                  onChange={(e) => {
                    const v = e.currentTarget.value.trim();
                    patch((c) => {
                      if (v) c.theme.tokens[k] = v;
                      else delete c.theme.tokens[k];
                    });
                  }}
                />
              </div>
            )}
          </For>
        </div>
      </Group>
    </Page>
  );
}
