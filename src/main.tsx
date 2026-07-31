import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// Tauri 2 前端入口：挂载 React 工作台
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
