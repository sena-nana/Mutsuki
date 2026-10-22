import { readFileSync } from "node:fs";
import "./bad.css";

export const read = () => readFileSync("x");
