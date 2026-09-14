import { render } from "solid-js/web";
import { App } from "./App";
import { initializeTelemetry } from "./telemetry";
import "./styles.css";

initializeTelemetry();

const root = document.getElementById("root");
if (!root) throw new Error("Missing application root");
render(() => <App />, root);
