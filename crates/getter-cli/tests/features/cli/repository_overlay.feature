@getter-cli @repository
Feature: Getter CLI repository overlay resolution
  Scenario: Highest-priority repository package wins by package id
    Given an initialized getter data directory
    And a fixture Lua repository "official" with package "android/org.fdroid.fdroid" named "Official F-Droid"
    And a fixture Lua repository "local" with package "android/org.fdroid.fdroid" named "Local F-Droid"
    When I run getter repo add for repository "official" with priority 0
    Then the command succeeds
    When I run getter repo add for repository "local" with priority 100
    Then the command succeeds
    When I run getter package eval for package "android/org.fdroid.fdroid"
    Then the command succeeds
    And the output is valid JSON
    And the output contains package "android/org.fdroid.fdroid" named "Local F-Droid"
