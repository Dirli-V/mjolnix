{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.mjolnix;

  mjolnixEnv = {
    MJOLNIX_DATA_DIR = cfg.dataDir;
    MJOLNIX_HOST = cfg.host;
    MJOLNIX_BIN = "${lib.getBin cfg.package}/mjolnix";
    MJOLNIX_MAX_PARALLEL_BUILDS = toString cfg.maxParallelBuilds;
    MJOLNIX_BUILD_TIMEOUT_SECS = toString cfg.buildTimeoutSecs;
  };

  substituterUrl =
    if cfg.binaryCache.enable then
      "http://${cfg.host}:${toString cfg.binaryCache.port}/"
    else
      null;

  mjolnixLoginShell = pkgs.writeScriptBin "mjolnix-login" ''
    #!${pkgs.runtimeShell}
    exec ${cfg.package}/bin/mjolnix
  '';
in
{
  options.services.mjolnix = {
    enable = lib.mkEnableOption "mjolnix git hosting with Nix builds";

    package = lib.mkOption {
      type = lib.types.package;
      default = lib.mkIf (lib.hasAttr "mjolnix" pkgs) pkgs.mjolnix;
      defaultText = lib.mkDefault "pkgs.mjolnix";
      description = "The mjolnix package. Use the flake overlay (`nixpkgs.overlays = [ inputs.mjolnix.overlays.default ];`) or set this to `inputs.mjolnix.packages.<system>.default`.";
    };

    dataDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/mjolnix";
      description = "State directory for repositories, database, build workdirs, and socket.";
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = config.networking.hostName;
      description = "Hostname shown in clone URLs (MJOLNIX_HOST).";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "git";
      description = "System user for SSH git access and builds.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "git";
      description = "Group for the mjolnix system user.";
    };

    authorizedKeys = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "SSH public keys allowed to log in as the mjolnix user.";
    };

    maxParallelBuilds = lib.mkOption {
      type = lib.types.int;
      default = 2;
      description = "Maximum concurrent Nix builds in mjolnixd.";
    };

    buildTimeoutSecs = lib.mkOption {
      type = lib.types.int;
      default = 3600;
      description = "Per-build timeout in seconds.";
    };

    openssh.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether to enable OpenSSH (required for git over SSH).";
    };

    binaryCache = {
      enable = lib.mkEnableOption "Harmonia binary cache for build outputs";

      port = lib.mkOption {
        type = lib.types.port;
        default = 5000;
        description = "TCP port for Harmonia.";
      };

      signKeyPath = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Optional path to Harmonia signing key.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.package != null;
        message = "services.mjolnix.package must be set. Apply the mjolnix flake overlay or set services.mjolnix.package explicitly.";
      }
    ];

    services.openssh = {
      enable = lib.mkIf cfg.openssh.enable (lib.mkDefault true);
      extraConfig = lib.mkAfter ''
        Match User ${cfg.user}
          SetEnv MJOLNIX_DATA_DIR=${cfg.dataDir}
          SetEnv MJOLNIX_HOST=${cfg.host}
          SetEnv MJOLNIX_BIN=${cfg.package}/bin/mjolnix
          SetEnv MJOLNIX_MAX_PARALLEL_BUILDS=${toString cfg.maxParallelBuilds}
          SetEnv MJOLNIX_BUILD_TIMEOUT_SECS=${toString cfg.buildTimeoutSecs}
          ${lib.optionalString (substituterUrl != null) "SetEnv MJOLNIX_SUBSTITUTER_URL=${substituterUrl}"}
      '';
    };

    users.groups.${cfg.group} = { };

    users.users.${cfg.user} = {
      description = "mjolnix git user";
      isSystemUser = true;
      group = cfg.group;
      extraGroups = [ "nixbld" ];
      home = cfg.dataDir;
      createHome = false;
      shell = lib.mkForce "${lib.getExe mjolnixLoginShell}";
      openssh.authorizedKeys.keys = cfg.authorizedKeys;
    };

    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.dataDir}/repos 0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.dataDir}/work 0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.dataDir}/logs 0750 ${cfg.user} ${cfg.group} -"
      "Z ${cfg.dataDir} - ${cfg.user} ${cfg.group} -"
    ];

    systemd.services.mjolnixd = {
      description = "mjolnix Nix build daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      wants = [ "network.target" ];

      environment = mjolnixEnv // lib.optionalAttrs (substituterUrl != null) {
        MJOLNIX_SUBSTITUTER_URL = substituterUrl;
      };

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        ExecStart = "${lib.getBin cfg.package}/bin/mjolnixd";
        Restart = "on-failure";
        RestartSec = "5s";
        # git + nix for builds triggered by the daemon
        Path = with pkgs; [
          git
          nix
          coreutils
        ];
      };
    };

    services.harmonia = lib.mkIf cfg.binaryCache.enable {
      enable = true;
      signKeyPath = cfg.binaryCache.signKeyPath;
      settings = {
        bind = [ "[::]:${toString cfg.binaryCache.port}" ];
        workers = 2;
      };
    };
  };
}
