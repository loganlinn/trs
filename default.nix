{
  lib,
  name,
  stdenv,
}:
stdenv.mkDerivation {
  inherit name;
  src = null;
  phases = [];
}
