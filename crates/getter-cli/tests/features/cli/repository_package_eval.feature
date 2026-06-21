@getter-cli @repository
Feature: Getter CLI repository and package evaluation
  Scenario: User adds and evaluates a fixture Lua repository
    Given an initialized getter data directory
    And a fixture Lua repository "official" with package "android/org.fdroid.fdroid"
    When I run getter repo add for that repository with priority 0
    Then the command succeeds
    And the output is valid JSON
    And the output contains the added repository
    When I run getter repo eval for that repository
    Then the command succeeds
    And the output is valid JSON
    And the output contains the evaluated fixture package
    When I run getter package eval for that fixture package
    Then the command succeeds
    And the output is valid JSON
    And the output contains the fixture package
