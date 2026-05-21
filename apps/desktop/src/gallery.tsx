import React from "react";
import ReactDOM from "react-dom/client";
import "./ui/tokens.css";
import { Gallery } from "./ui/Gallery";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Gallery />
  </React.StrictMode>,
);
