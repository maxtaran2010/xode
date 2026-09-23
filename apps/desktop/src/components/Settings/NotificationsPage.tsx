import { notify } from "../../lib/api";
import type { Notifications } from "../../lib/types";
import { cfg, Field, Group, Page, patch, Toggle } from "./controls";

export default function NotificationsPage() {
  const n = () => cfg().notifications;
  const set = <K extends keyof Notifications>(k: K, v: Notifications[K]) => patch((c) => (c.notifications[k] = v));
  const row = (k: keyof Notifications, label: string) => (
    <Field label={label}>
      <Toggle value={n()[k]} onChange={(v) => set(k, v)} />
    </Field>
  );
  return (
    <Page
      title="Notifications"
      actions={
        <button class="btn sm" onClick={() => notify("Xode", "Test notification", n().sound)}>
          Test
        </button>
      }
    >
      <Group title="Notify when">
        {row("permission", "Permission needed")}
        {row("finished", "Work finished")}
        {row("errors", "Run failed")}
      </Group>
      <Group>
        {row("sound", "Sound")}
        {row("only_unfocused", "Only when in background")}
      </Group>
    </Page>
  );
}
