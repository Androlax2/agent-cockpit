import { manifest } from "../lib/source";
export const GET = () =>
  new Response(JSON.stringify(manifest), { headers: { "Content-Type": "application/json" } });
