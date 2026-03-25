"""PCAT Pipeline Server — FastAPI sidecar for the Tauri desktop app."""

import socket
import sys

import uvicorn
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from dicom_routes import router as dicom_router

app = FastAPI(title="PCAT Pipeline Server", version="0.1.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

# Wire in DICOM loading endpoints
app.include_router(dicom_router)


@app.get("/ping")
async def ping():
    """Health check endpoint."""
    return {"status": "ok"}


def find_free_port() -> int:
    """Find a free TCP port on localhost."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def main():
    port = find_free_port()
    # Signal the port to the parent process (Rust reads this)
    print(f"PORT:{port}", flush=True)
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")


if __name__ == "__main__":
    main()
