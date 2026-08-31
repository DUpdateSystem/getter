@getter-cli @smoke
Feature: Getter CLI initialization
  Scenario: User initializes a new getter data directory
    Given an empty getter data directory
    When I run getter init for that directory
    Then the command succeeds
    And the output is valid JSON
    And the getter data directory is usable
