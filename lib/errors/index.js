import { Module } from "../module.js";
export const isError = (code) => {
  const _isError = Module["_ZSTD_isError"];
  return _isError(code);
};
