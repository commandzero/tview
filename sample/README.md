# Sample data

## `us-counties.sqlite3`

`us-counties.sqlite3` is a small, read-only example database built from the
U.S. Census Bureau's 2010 to 2020 county population estimates. Work created by
Census Bureau employees is [generally not subject to copyright][public-access],
and the Bureau publishes its public data as open data.

- Source: [County Population Totals: 2010 to 2020][dataset]
- Source file: [Annual Resident Population Estimates for Counties][csv]
- Selection: the 1,000 most populous county-level records with `SUMLEV=050`, ordered by
  `POPESTIMATE2020`, descending, with numeric state/county FIPS as the
  tie-breaker
- Table: `counties`
- Shape: 1,000 rows and 16 columns
- SHA-256:
  `7ccee2b9d5ec10bc365f6b9cf1063e94ccb8d4c7cc0e3cc6cbe58b91c1c32ad4`

The columns preserve county identity and geography alongside selected Census
2010 counts and 2015, 2019, and 2020 estimates. The 2020 population change,
births, deaths, domestic migration, and net migration rate fields contain
positive, negative, and floating-point values for filtering, sorting, and
formatting examples.

Open it with:

```sh
tview sample/us-counties.sqlite3
```

Or select the table explicitly:

```sh
tview sample/us-counties.sqlite3 --table counties
```

This sample is a subset and repackaging of Census Bureau data. It is not
endorsed or certified by the Census Bureau.

[dataset]: https://www.census.gov/programs-surveys/popest/technical-documentation/research/evaluation-estimates/2020-evaluation-estimates/2010s-counties-total.html
[csv]: https://www2.census.gov/programs-surveys/popest/datasets/2010-2020/counties/totals/co-est2020-alldata.csv
[public-access]: https://www2.census.gov/foia/ds_policies/ds027.pdf
