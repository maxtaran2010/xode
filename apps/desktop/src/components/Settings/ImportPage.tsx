import { createResource, Show } from "solid-js";
import { RefreshCw } from "lucide-solid";
import { api } from "../../lib/api";
import type { ImportSource } from "../../lib/types";
import ImportRows from "../ImportRows";
import { Spinner } from "../ui";
import { Group, Page } from "./controls";

export default function ImportPage() {
  const [sources, { refetch }] = createResource(() => api.importDetect().catch(() => [] as ImportSource[]));
  return (
    <Page
      title="Import"
      actions={
        <button class="btn sm" onClick={() => refetch()}>
          <RefreshCw size={14} stroke-width={1.7} />
          Rescan
        </button>
      }
    >
      <Group title="From other agents">
        <Show when={!sources.loading} fallback={<div class="ob-loading"><Spinner size={16} /></div>}>
          <ImportRows sources={sources() ?? []} />
        </Show>
      </Group>
    </Page>
  );
}
