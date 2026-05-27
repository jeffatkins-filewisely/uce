/**
 * REFERENCE ONLY — merge heartbeat branch into your production uce-ingest.
 * See MERGE_HEARTBEAT.md
 */
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.49.1";
import { handleUceHeartbeat, isHeartbeatBody } from "../_shared/uceHeartbeat.ts";

const corsHeaders = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers":
    "authorization, x-client-info, apikey, content-type",
};

Deno.serve(async (req) => {
  if (req.method === "OPTIONS") {
    return new Response("ok", { headers: corsHeaders });
  }

  const supabase = createClient(
    Deno.env.get("SUPABASE_URL")!,
    Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!,
  );

  let body: Record<string, unknown>;
  try {
    body = await req.json();
  } catch {
    return new Response(JSON.stringify({ error: "invalid json" }), {
      status: 400,
      headers: { ...corsHeaders, "Content-Type": "application/json" },
    });
  }

  if (isHeartbeatBody(body)) {
    const { status, body: respBody } = await handleUceHeartbeat(supabase, body);
    return new Response(JSON.stringify(respBody), {
      status,
      headers: { ...corsHeaders, "Content-Type": "application/json" },
    });
  }

  // TODO: delegate to your existing capture handler
  return new Response(
    JSON.stringify({
      error: "reference stub — wire capture path from production uce-ingest",
    }),
    {
      status: 501,
      headers: { ...corsHeaders, "Content-Type": "application/json" },
    },
  );
});
