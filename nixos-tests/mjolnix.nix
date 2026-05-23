# NixOS integration test for services.mjolnix
{
  package,
  pkgs,
  lib,
}:

pkgs.testers.runNixOSTest {
  name = "mjolnix";

  nodes.machine = {
    config,
    lib,
    pkgs,
    ...
  }:
  {
    imports = [
      ../nix/modules/mjolnix.nix
    ];

    networking.hostName = "mjolnix-test";

    services.mjolnix = {
      enable = true;
      inherit package;
      host = "mjolnix-test";
      binaryCache.enable = false;
      authorizedKeys = [
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHRlc3Qta2V5IGZvciBuaXhvcy10ZXN0IG9ubHk="
      ];
    };

    services.openssh.enable = true;

    # Nix is required for mjolnixd builds
    nix.enable = true;

    environment.systemPackages = with pkgs; [ git ];
  };

  testScript = ''
    machine.wait_for_unit("mjolnixd.service")
    machine.wait_for_open_port(22)

    machine.succeed("test -S /var/lib/mjolnix/mjolnixd.sock")
    machine.succeed("test -d /var/lib/mjolnix/repos")
    machine.succeed("id git")

    machine.succeed("grep -q 'SetEnv MJOLNIX_DATA_DIR' /etc/ssh/sshd_config")
    machine.succeed("grep -q 'SetEnv MJOLNIX_DATABASE_URL' /etc/ssh/sshd_config")
    machine.wait_for_unit("postgresql.service")
    machine.succeed("test -x ${package}/bin/mjolnix")
    machine.succeed("test -x ${package}/bin/mjolnixd")
    '';
}
