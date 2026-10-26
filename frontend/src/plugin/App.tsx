import { useEffect } from "react";
import { Canvas } from "./Canvas";
import { useCanvas } from "./store";

/**
 * The surface root. Starts the engine polling on mount and shows a hint while
 * the fleet is empty, over the board so pan and zoom still work.
 */
export function App() {
  const init = useCanvas((s) => s.init);
  const loaded = useCanvas((s) => s.loaded);
  const empty = useCanvas((s) => s.nodes.length === 0);

  useEffect(() => init(), [init]);

  return (
    <div className="app">
      <Canvas />
      {loaded && empty && <div className="hint">No agent sessions yet — launch one in Goia and it appears here.</div>}
    </div>
  );
}
