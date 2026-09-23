import { fmtTokens } from "../lib/format";

export default function ContextRing(props: { used: number; limit: number; threshold: number; onClick: () => void }) {
  const r = 7;
  const c = 2 * Math.PI * r;
  const frac = () => (props.limit > 0 ? Math.min(1, props.used / props.limit) : 0);
  const warn = () => props.threshold > 0 && props.used >= props.threshold * 0.9;
  return (
    <button class="ctx-ring" classList={{ warn: warn() }} onClick={props.onClick} data-tip={`${fmtTokens(props.used)} / ${fmtTokens(props.limit)}`}>
      <svg width="18" height="18" viewBox="0 0 18 18">
        <circle cx="9" cy="9" r={r} fill="none" stroke="currentColor" opacity="0.3" stroke-width="2" />
        <circle
          cx="9"
          cy="9"
          r={r}
          fill="none"
          stroke="currentColor"
          stroke-width="2"
                    stroke-dasharray={`${c}`}
          stroke-dashoffset={`${c * (1 - frac())}`}
          transform="rotate(-90 9 9)"
        />
      </svg>
    </button>
  );
}
