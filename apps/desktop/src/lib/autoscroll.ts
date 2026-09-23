// Stick-to-bottom scrolling: follows new content until the user scrolls up (even slightly),
// and re-attaches once they scroll back to the bottom.
import { createSignal, onCleanup } from "solid-js";

const EPS = 3;

/** Call from a component (e.g. in a `ref` callback). Returns the stuck state and a jump helper. */
export function stickToBottom(el: HTMLElement) {
  const [stuck, setStuck] = createSignal(true);
  let lastSet = -1;
  let raf = 0;

  const distance = () => el.scrollHeight - el.scrollTop - el.clientHeight;
  const toBottom = () => {
    lastSet = el.scrollHeight - el.clientHeight;
    el.scrollTop = el.scrollHeight;
  };
  const follow = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      if (stuck() && distance() > 0) toBottom();
    });
  };

  const onScroll = () => {
    // Our own scroll (or the browser clamping it) keeps the current state.
    if (Math.abs(el.scrollTop - lastSet) < 1) return;
    lastSet = -1;
    setStuck(distance() <= EPS);
  };
  const onWheel = (e: WheelEvent) => {
    if (e.deltaY < 0 && el.scrollHeight > el.clientHeight) {
      lastSet = -1;
      setStuck(false);
    }
  };
  const onKey = (e: KeyboardEvent) => {
    if (["ArrowUp", "PageUp", "Home"].includes(e.key)) {
      lastSet = -1;
      setStuck(false);
    }
  };
  let touchY = 0;
  const onTouchStart = (e: TouchEvent) => (touchY = e.touches[0]?.clientY ?? 0);
  const onTouchMove = (e: TouchEvent) => {
    if ((e.touches[0]?.clientY ?? 0) > touchY + 2) setStuck(false);
  };

  el.addEventListener("scroll", onScroll, { passive: true });
  el.addEventListener("wheel", onWheel, { passive: true });
  el.addEventListener("keydown", onKey);
  el.addEventListener("touchstart", onTouchStart, { passive: true });
  el.addEventListener("touchmove", onTouchMove, { passive: true });

  // Streaming text changes don't always resize the scroller (fixed max-height), so watch the DOM too.
  const mo = new MutationObserver(follow);
  mo.observe(el, { childList: true, subtree: true, characterData: true });
  const ro = new ResizeObserver(follow);
  ro.observe(el);
  for (const c of Array.from(el.children)) ro.observe(c);

  requestAnimationFrame(toBottom);

  onCleanup(() => {
    cancelAnimationFrame(raf);
    mo.disconnect();
    ro.disconnect();
    el.removeEventListener("scroll", onScroll);
    el.removeEventListener("wheel", onWheel);
    el.removeEventListener("keydown", onKey);
    el.removeEventListener("touchstart", onTouchStart);
    el.removeEventListener("touchmove", onTouchMove);
  });

  return {
    stuck,
    /** Scroll to the bottom and follow again. */
    jump: () => {
      setStuck(true);
      toBottom();
    },
  };
}
