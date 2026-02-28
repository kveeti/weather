{ weatherPkg }:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.weather;
  dataDir = "/var/lib/weather";
in
{
  options.services.weather = {
    enable = lib.mkEnableOption "Weather Service";

    environment = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = {};
      description = "Environment variables to pass to the service";
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "File containing environment variables to pass to the service";
    };
  };

  config = lib.mkIf cfg.enable {
    users.groups.weather = { };
    users.users.weather = {
      isSystemUser = true;
      group = "weather";
    };

    systemd.services.weather = {
      description = "Weather Service";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      environment = cfg.environment;

      serviceConfig = {
        ExecStart = "${weatherPkg}/bin/weather";
        Restart = "always";
        User = "weather";
        Group = "weather";

        StateDirectory = "weather";
        WorkingDirectory = dataDir;

        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;

        # Hardening
        CapabilityBoundingSet = [ "" ];
        DeviceAllow = [ "/dev/stdin" "/dev/urandom" ];
        DevicePolicy = "strict";
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        PrivateUsers = true;
        ProcSubset = "pid";
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectProc = "invisible";
        ProtectSystem = "strict";
        RemoveIPC = true;
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" "~@resources" ];
        UMask = "0027";
      };
    };
  };
}
