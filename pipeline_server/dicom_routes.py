"""DICOM loading endpoints for the PCAT pipeline server."""

from __future__ import annotations

import sys
import uuid
from pathlib import Path
from typing import List, Optional

import numpy as np
from fastapi import APIRouter, HTTPException, Query
from fastapi.responses import Response
from pydantic import BaseModel

# Add the pipeline package to sys.path so we can import dicom_loader
_pipeline_dir = str(Path(__file__).resolve().parent.parent.parent / "pipeline")
if _pipeline_dir not in sys.path:
    sys.path.insert(0, _pipeline_dir)

from volume_manager import LoadedVolume, volumes  # noqa: E402

router = APIRouter()


# ── Request / Response models ────────────────────────────────────────────

class ScanRequest(BaseModel):
    path: str


class SeriesInfo(BaseModel):
    series_uid: str
    series_description: str
    modality: str
    num_images: int
    patient_name: str
    study_description: str


class ScanResponse(BaseModel):
    series: List[SeriesInfo]


class LoadRequest(BaseModel):
    path: str


class LoadResponse(BaseModel):
    volume_id: str
    shape: List[int]
    spacing: List[float]
    origin: List[float]
    patient_name: str
    study_description: str
    series_description: str
    window_center: float
    window_width: float


# ── Endpoints ────────────────────────────────────────────────────────────

@router.post("/scan_dicom", response_model=ScanResponse)
async def scan_dicom(req: ScanRequest):
    """Scan a directory for DICOM series.

    Walks the directory, groups files by SeriesInstanceUID, and returns
    a summary of each series found.
    """
    import pydicom

    scan_path = Path(req.path)
    if not scan_path.exists():
        raise HTTPException(status_code=404, detail=f"Path not found: {req.path}")
    if not scan_path.is_dir():
        raise HTTPException(status_code=400, detail=f"Path is not a directory: {req.path}")

    # Collect DICOM files grouped by SeriesInstanceUID
    series_map: dict[str, list] = {}
    for f in scan_path.rglob("*"):
        if f.is_dir():
            continue
        try:
            ds = pydicom.dcmread(str(f), stop_before_pixels=True)
            uid = str(getattr(ds, "SeriesInstanceUID", "unknown"))
            series_map.setdefault(uid, []).append(ds)
        except Exception:
            continue  # skip non-DICOM files

    results: List[SeriesInfo] = []
    for uid, datasets in series_map.items():
        ref = datasets[0]
        results.append(SeriesInfo(
            series_uid=uid,
            series_description=str(getattr(ref, "SeriesDescription", "")),
            modality=str(getattr(ref, "Modality", "")),
            num_images=len(datasets),
            patient_name=str(getattr(ref, "PatientName", "")),
            study_description=str(getattr(ref, "StudyDescription", "")),
        ))

    return ScanResponse(series=results)


@router.post("/load_dicom", response_model=LoadResponse)
async def load_dicom(req: LoadRequest):
    """Load a DICOM series from a directory into memory.

    Uses the pipeline's dicom_loader.load_dicom_series() to read the volume,
    converts to int16 HU, stores it in the VolumeManager, and returns metadata.
    """
    import dicom_loader  # from pipeline directory added to sys.path above

    load_path = Path(req.path)
    if not load_path.exists():
        raise HTTPException(status_code=404, detail=f"Path not found: {req.path}")
    if not load_path.is_dir():
        raise HTTPException(status_code=400, detail=f"Path is not a directory: {req.path}")

    try:
        volume_f32, meta = dicom_loader.load_dicom_series(str(load_path))
    except FileNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Failed to load DICOM: {e}")

    # Convert float32 HU → int16 for compact storage and fast slice serving
    volume_i16 = np.clip(volume_f32, -32768, 32767).astype(np.int16)

    vol_id = uuid.uuid4().hex[:12]

    # Determine default window from the data (soft-tissue window as fallback)
    window_center = 40.0
    window_width = 400.0

    loaded = LoadedVolume(
        volume_id=vol_id,
        data=volume_i16,
        spacing=meta.get("spacing_mm", [1.0, 1.0, 1.0]),
        origin=meta.get("origin_mm", [0.0, 0.0, 0.0]),
        direction=meta.get("orientation", [1, 0, 0, 0, 1, 0]),
        window_center=window_center,
        window_width=window_width,
        patient_name=meta.get("patient_id", ""),
        study_description=meta.get("study_description", ""),
    )
    volumes.store(loaded)

    return LoadResponse(
        volume_id=vol_id,
        shape=list(volume_i16.shape),
        spacing=loaded.spacing,
        origin=loaded.origin,
        patient_name=loaded.patient_name,
        study_description=loaded.study_description,
        series_description=meta.get("series_description", ""),
        window_center=window_center,
        window_width=window_width,
    )


@router.get("/volume/{volume_id}/slice")
async def get_volume_slice(
    volume_id: str,
    axis: str = Query("axial", description="axial, coronal, or sagittal"),
    idx: int = Query(0, description="Slice index along the chosen axis"),
):
    """Return a single 2D slice as raw int16 bytes.

    The response body is raw binary (application/octet-stream) containing
    int16 little-endian pixel values. The client must know the slice
    dimensions from the volume metadata to reshape the data.
    """
    try:
        raw_bytes = volumes.get_slice(volume_id, axis, idx)
    except KeyError:
        raise HTTPException(status_code=404, detail=f"Volume {volume_id} not found")
    except IndexError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    return Response(content=raw_bytes, media_type="application/octet-stream")
