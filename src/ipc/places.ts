import { invoke } from "@tauri-apps/api/core";

export interface Place {
  id: number;
  name: string;
  lat: number;
  lon: number;
  radiusM: number;
  createdMs: number;
}

export function listPlaces(): Promise<Place[]> {
  return invoke<Place[]>("list_places");
}

export function addPlace(
  name: string,
  lat: number,
  lon: number,
  radiusM: number,
): Promise<number> {
  return invoke<number>("add_place", { name, lat, lon, radiusM });
}

export function updatePlace(
  id: number,
  name: string,
  lat: number,
  lon: number,
  radiusM: number,
): Promise<void> {
  return invoke<void>("update_place", { id, name, lat, lon, radiusM });
}

export function deletePlace(id: number): Promise<void> {
  return invoke<void>("delete_place", { id });
}
