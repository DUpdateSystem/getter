Feature: Getter CLI repository validation
  Scenario: User validates a fixture Lua repository offline
    Given an initialized getter data directory
    And a fixture Lua repository "official" with package "android/org.fdroid.fdroid"
    When I run getter repo validate for that repository
    Then the command succeeds
    And the output reports a valid repository without network

  Scenario: User receives diagnostics for invalid Lua
    Given an initialized getter data directory
    And a fixture Lua repository "broken" with invalid Lua package "android/org.fdroid.fdroid"
    When I run getter repo validate for that repository
    Then the command succeeds
    And the output reports repository diagnostic "package.lua_runtime"

  Scenario: User receives diagnostics for invalid package schema
    Given an initialized getter data directory
    And a fixture Lua repository "broken" with schema-invalid package "android/org.fdroid.fdroid"
    When I run getter repo validate for that repository
    Then the command succeeds
    And the output reports repository diagnostic "package.schema"

  Scenario: User receives diagnostics for package path mismatch
    Given an initialized getter data directory
    And a fixture Lua repository "broken" with mismatched package path "android/org.fdroid.fdroid"
    When I run getter repo validate for that repository
    Then the command succeeds
    And the output reports repository diagnostic "package.domain"

  Scenario: User receives diagnostics for an incomplete repository layout
    Given an initialized getter data directory
    And an incomplete Lua repository "broken"
    When I run getter repo validate for that repository
    Then the command succeeds
    And the output reports repository diagnostic "repository.missing_directory"
