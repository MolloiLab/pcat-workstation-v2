"""Volume manager — stores loaded CT volumes in memory and serves slices."""

from __future__ import annotations

import uuid
from dataclasses import dataclass, field
from typing import Dict, List, Optional

import numpy as np


@dataclass
class LoadedVolume:
    """A CT volume loaded into memory, ready for slice serving."""

    volume_id: str
    data: np.ndarray  # int16, shape (Z, Y, X) in HU
    spacing: List[float]  # [z, y, x] in mm
    origin: List[float]  # [x, y, z] patient coords mm
    direction: List[float]  # ImageOrientationPatient 6-element list
    window_center: float = 40.0
    window_width: float = 400.0
    patient_name: str = ""
    study_description: str = ""
    shape: List[int] = field(default_factory=list)

    def __post_init__(self):
        if not self.shape:
            self.shape = list(self.data.shape)


class VolumeManager:
    """In-memory store for loaded volumes."""

    def __init__(self) -> None:
        self._volumes: Dict[str, LoadedVolume] = {}

    def store(self, volume: LoadedVolume) -> str:
        """Store a volume and return its ID."""
        self._volumes[volume.volume_id] = volume
        return volume.volume_id

    def get(self, volume_id: str) -> Optional[LoadedVolume]:
        """Retrieve a volume by ID, or None if not found."""
        return self._volumes.get(volume_id)

    def get_slice(self, volume_id: str, axis: str, idx: int) -> bytes:
        """Extract a 2D slice as raw int16 bytes.

        Parameters
        ----------
        volume_id : str
        axis : str — "axial", "coronal", or "sagittal"
        idx : int — slice index along the chosen axis

        Returns
        -------
        bytes — raw int16 little-endian pixel data
        """
        vol = self._volumes.get(volume_id)
        if vol is None:
            raise KeyError(f"Volume {volume_id} not found")

        data = vol.data  # shape (Z, Y, X), already int16

        if axis == "axial":
            if idx < 0 or idx >= data.shape[0]:
                raise IndexError(f"Axial index {idx} out of range [0, {data.shape[0]})")
            slc = data[idx, :, :]
        elif axis == "coronal":
            if idx < 0 or idx >= data.shape[1]:
                raise IndexError(f"Coronal index {idx} out of range [0, {data.shape[1]})")
            slc = data[:, idx, :]
        elif axis == "sagittal":
            if idx < 0 or idx >= data.shape[2]:
                raise IndexError(f"Sagittal index {idx} out of range [0, {data.shape[2]})")
            slc = data[:, :, idx]
        else:
            raise ValueError(f"Unknown axis '{axis}'. Use axial, coronal, or sagittal.")

        # Ensure contiguous C-order int16 for raw byte transfer
        slc = np.ascontiguousarray(slc, dtype=np.int16)
        return slc.tobytes()

    def remove(self, volume_id: str) -> bool:
        """Remove a volume from memory. Returns True if it existed."""
        return self._volumes.pop(volume_id, None) is not None

    def list_ids(self) -> List[str]:
        """Return all stored volume IDs."""
        return list(self._volumes.keys())


# Module-level singleton
volumes = VolumeManager()
