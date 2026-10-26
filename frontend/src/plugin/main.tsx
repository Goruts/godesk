import { createRoot } from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";
import { App } from "./App";

// No StrictMode: it double-invokes effects, which would start the engine polling
// twice. A surface is a plain app, not a tree under test.
const root = document.getElementById("root");
if (root) createRoot(root).render(<App />);
