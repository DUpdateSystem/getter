@getter-cli @smoke
Feature: Getter CLI storage validation
  Scenario: User validates initialized getter storage
    Given an initialized getter data directory
    When I run getter storage validate for that directory
    Then the command succeeds
    And the output is valid JSON
    And the output reports valid storage
