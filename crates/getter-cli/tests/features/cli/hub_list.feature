@getter-cli @smoke
Feature: Getter CLI hub listing
  Scenario: User lists hubs before adding any hub records
    Given an initialized getter data directory
    When I run getter hub list for that directory
    Then the command succeeds
    And the output is valid JSON
    And the output contains an empty hub list
