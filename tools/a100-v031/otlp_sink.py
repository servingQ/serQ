#!/usr/bin/env python3
"""OTLP/HTTP trace receiver: one JSON line per span (vLLM's `llm_request`).

usage: otlp_sink.py PORT OUT.jsonl   (vLLM: OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf,
                                      --otlp-traces-endpoint http://127.0.0.1:PORT/v1/traces)
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest

port, out = int(sys.argv[1]), open(sys.argv[2], "a")


def val(v):
    k = v.WhichOneof("value")
    return getattr(v, k) if k else None


class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        req = ExportTraceServiceRequest()
        req.ParseFromString(body)
        for rs in req.resource_spans:
            for ss in rs.scope_spans:
                for s in ss.spans:
                    out.write(json.dumps({
                        "name": s.name,
                        "start_ns": s.start_time_unix_nano,
                        "end_ns": s.end_time_unix_nano,
                        "attrs": {a.key: val(a.value) for a in s.attributes},
                    }) + "\n")
        out.flush()
        self.send_response(200)
        self.send_header("Content-Type", "application/x-protobuf")
        self.end_headers()

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", port), H).serve_forever()
