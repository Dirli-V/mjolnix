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
      binaryCache.enable = true;
      binaryCache.bind = "0.0.0.0:5000";
      authorizedKeys = [
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHRlc3Qta2V5IGZvciBuaXhvcy10ZXN0IG9ubHk="
      ];
    };

    services.openssh.enable = true;

    nix.enable = true;

    environment.systemPackages = with pkgs; [git];
  };

  testScript = ''
    machine.wait_for_unit("postgresql.service")
    machine.wait_for_unit("mjolnix-postgresql-init.service")
    machine.wait_for_unit("mjolnix-worker.service")
    machine.wait_for_unit("mjolnix-cache.service")
    machine.wait_for_open_port(5000)
    machine.wait_for_open_port(22)

    machine.succeed("test -d /var/lib/mjolnix/repos")
    machine.succeed("test -d /var/lib/mjolnix/stores")
    machine.succeed("id git")

    machine.succeed("grep -q 'SetEnv MJOLNIX_DATA_DIR' /etc/ssh/sshd_config")
    machine.succeed("grep -q 'SetEnv MJOLNIX_DATABASE_URL' /etc/ssh/sshd_config")
    machine.wait_for_unit("postgresql.service")
    machine.succeed("test -x ${package}/bin/mjolnix-frontend")
    machine.succeed("test -x ${package}/bin/mjolnix-worker")
    machine.succeed("test -x ${package}/bin/mjolnix-cache")
    machine.succeed("test -x ${package}/bin/mjolnix")
  '';
}
